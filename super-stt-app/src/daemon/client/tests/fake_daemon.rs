// SPDX-License-Identifier: GPL-3.0-only
//! A daemon that isn't one: an in-process HTTP/1.1 server on a Unix socket,
//! so every wrapper under `v1/` can be run for real and asked what it sent.
//!
//! [`super::super::path_contract`] already checks that each path the client
//! writes is a path the daemon serves. The rest of the request was checked by
//! nothing: which verb it uses, which JSON keys go in the body, whether a
//! `source` containing a `/` is percent-encoded before it lands in the URL.
//! None of that is visible to either crate's type system — the two sides share
//! no type for the request, only the bytes — and a mistake in it surfaces as a
//! `404` or a `not_found` that reads like bad input rather than a client bug.
//!
//! So: call the real function, let it build a real request over the real
//! transport, assert on what arrives, hand back a canned response, and assert
//! on what the function made of it.
//!
//! Two process-wide things make the redirection work, and are why a test holds
//! [`ONLY_ONE`] for its whole life: the socket the client resolves
//! ([`test_socket`]) and the session-token cache. The token is seeded rather
//! than minted, so no call reaches the consent popup.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Once};

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;

use crate::daemon::client::internal::session::{APP_ID_NAME, test_socket};

/// The bearer token every wrapper presents while a fake daemon is up.
pub(crate) const TOKEN: &str = "fake-daemon-token";

/// Serializes the wrapper tests. Both the socket the client resolves and the
/// session-token cache are process-wide, so only one fake daemon can be *the*
/// daemon at a time.
///
/// Tokio's mutex rather than `std`'s for two reasons: it is held across the
/// awaits of the call under test, and it does not poison, so one failed
/// assertion doesn't take every later test in the file down with it.
static ONLY_ONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One request as it arrived.
#[derive(Debug, Clone)]
pub(crate) struct Recorded {
    pub method: String,
    /// The request target as sent — transport `/v1` prefix and query string
    /// included.
    pub target: String,
    pub authorization: Option<String>,
    pub body: Bytes,
}

impl Recorded {
    /// The path the wrapper wrote: the target with the transport's `/v1`
    /// prefix and any query string removed, which is what the call site's own
    /// literal says.
    pub fn path(&self) -> &str {
        let target = self.target.split('?').next().unwrap_or(&self.target);
        target.strip_prefix("/v1").unwrap_or(target)
    }

    /// The query string, without the `?`. Empty when there is none.
    pub fn query(&self) -> &str {
        self.target.split_once('?').map_or("", |(_, query)| query)
    }

    /// The body as JSON; `Null` when there is no body or it isn't JSON.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// A canned response, queued before the call that should receive it.
pub(crate) struct Reply {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Reply {
    /// `200` with a JSON body.
    pub fn json(body: &serde_json::Value) -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: serde_json::to_vec(body).expect("reply body serializes"),
        }
    }

    /// The bare success envelope a write gets back. Also what an unscripted
    /// request is answered with.
    pub fn ok() -> Self {
        Self::json(&serde_json::json!({ "status": "success" }))
    }

    /// A non-2xx answer, for the error paths each wrapper maps.
    pub fn status(status: u16, body: &serde_json::Value) -> Self {
        Self {
            status,
            ..Self::json(body)
        }
    }

    /// A response with a body and content type of the caller's choosing —
    /// `text/event-stream` for the SSE paths, or a body that isn't JSON at all.
    pub fn raw(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type,
            body: body.into(),
        }
    }
}

#[derive(Default)]
struct Served {
    recorded: Mutex<Vec<Recorded>>,
    replies: Mutex<VecDeque<Reply>>,
}

/// A running fake daemon. Alive until dropped, which also releases
/// [`ONLY_ONE`] and puts the client's socket resolution back.
pub(crate) struct FakeDaemon {
    /// Held, not read: this is what keeps the next test out.
    _only_one: tokio::sync::MutexGuard<'static, ()>,
    dir: PathBuf,
    served: Arc<Served>,
    accept: tokio::task::JoinHandle<()>,
}

impl FakeDaemon {
    /// Bind a socket, start serving, and point the client at it.
    pub async fn start() -> Self {
        let only_one = ONLY_ONE.lock().await;
        install_mock_keyring();
        // Seed the in-memory token cache: `obtain` returns from it before any
        // keyring read or consent popup. `save` writes through to the keyring
        // on its way, which is why the mock has to be installed first.
        let _ = super_stt_shared::daemon::session::save(APP_ID_NAME, TOKEN);

        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "super-stt-app-fake-daemon-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create fake daemon dir");
        let socket = dir.join("http.sock");
        let listener = tokio::net::UnixListener::bind(&socket).expect("bind fake daemon socket");

        let served = Arc::new(Served::default());
        let accept = tokio::spawn({
            let served = Arc::clone(&served);
            async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let served = Arc::clone(&served);
                    tokio::spawn(async move {
                        let service =
                            service_fn(move |request| handle(Arc::clone(&served), request));
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
            }
        });

        test_socket::set(Some(socket));
        Self {
            _only_one: only_one,
            dir,
            served,
            accept,
        }
    }

    /// Queue the next response. Requests are answered in the order they
    /// arrive; an unscripted one gets [`Reply::ok`], which is all a write
    /// needs.
    pub fn reply(&self, reply: Reply) -> &Self {
        self.served
            .replies
            .lock()
            .expect("reply queue")
            .push_back(reply);
        self
    }

    /// Every request this daemon has served, in order.
    pub fn requests(&self) -> Vec<Recorded> {
        self.served.recorded.lock().expect("recorded").clone()
    }

    /// Take the socket away, leaving the client pointed at a path nothing is
    /// listening on — which is what a stopped daemon looks like from here.
    pub fn stop_listening(&self) {
        self.accept.abort();
        let _ = std::fs::remove_file(self.dir.join("http.sock"));
    }

    /// The single request the call made. Panics if it made any other number —
    /// a wrapper that retried, or that fired a second call nobody asked for,
    /// is a finding and not something to average over.
    pub fn request(&self) -> Recorded {
        let mut requests = self.requests();
        assert_eq!(
            requests.len(),
            1,
            "expected exactly one request, got {:?}",
            requests
                .iter()
                .map(|r| format!("{} {}", r.method, r.target))
                .collect::<Vec<_>>()
        );
        requests.pop().expect("one request")
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        test_socket::set(None);
        self.accept.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn handle(
    served: Arc<Served>,
    request: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, std::convert::Infallible> {
    let method = request.method().to_string();
    let target = request
        .uri()
        .path_and_query()
        .map_or_else(|| request.uri().path().to_string(), ToString::to_string);
    let authorization = request
        .headers()
        .get(hyper::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let body = request
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();

    served.recorded.lock().expect("recorded").push(Recorded {
        method,
        target,
        authorization,
        body,
    });

    let reply = served
        .replies
        .lock()
        .expect("reply queue")
        .pop_front()
        .unwrap_or_else(Reply::ok);

    Ok(hyper::Response::builder()
        .status(reply.status)
        .header(hyper::header::CONTENT_TYPE, reply.content_type)
        .body(Full::new(Bytes::from(reply.body)))
        .expect("fake daemon response builds"))
}

/// Route this process's keyring at the in-memory mock, once. The session
/// module writes through to the keyring on every `save`, and an automated run
/// has no business touching the developer's own secret service — nor, in CI,
/// a secret service that isn't there.
fn install_mock_keyring() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    });
}
