// SPDX-License-Identifier: GPL-3.0-only
//! Transport-level protocol conformance for the daemon's HTTP surface —
//! the responses to *malformed* requests, as opposed to the domain errors
//! (`invalid_session`, `scope_denied`, unknown model/backend) the other
//! smoke tests cover. None of these depend on daemon state, so they're
//! fully hermetic:
//!
//! - unknown route                 → `404`
//! - known route, wrong method     → `405` (both with and without auth middleware in front)
//! - malformed / missing JSON body → `400`
//!
//! Uses `SUPER_STT_AUTO_APPROVE=1` (no GUI) + `SUPER_STT_KEYRING_MOCK=1`
//! (in-memory keyring), so it runs in the default `cargo test` flow.

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1::handshake;
use hyper::{Method, Request, StatusCode};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use super_stt_shared::daemon::http_client;
use tokio::net::UnixStream;
use tokio::time::sleep;

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-stt-daemon");

struct DaemonGuard {
    child: Child,
    cleanup_paths: Vec<PathBuf>,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for p in &self.cleanup_paths {
            let _ = std::fs::remove_file(p);
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

fn next_test_uniq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static UNIQ: AtomicU64 = AtomicU64::new(0);
    UNIQ.fetch_add(1, Ordering::Relaxed)
}

async fn start_daemon() -> (DaemonGuard, PathBuf) {
    let unique = format!("stt-transport-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    std::fs::create_dir_all(&config_home).expect("create test config dir");
    let data_home = tmp.join(format!("{unique}-data"));
    std::fs::create_dir_all(&data_home).expect("create test data dir");

    let child = Command::new(DAEMON_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1")
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", &http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && http_client::auth_request(http_socket.clone(), "transport-smoke", &["status"])
                .await
                .is_ok()
        {
            return (
                DaemonGuard {
                    child,
                    cleanup_paths: vec![http_socket.clone(), config_home, data_home],
                },
                http_socket,
            );
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("daemon HTTP listener not ready within 120s");
}

/// Low-level request: any method/path, optional bearer token, optional body
/// + content-type. Returns `(status, raw body bytes)` — callers parse JSON
/// only when the response is expected to carry the daemon's error shape
/// (404/405 bodies are empty or framework-generated text).
async fn send(
    sock: &PathBuf,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<&[u8]>,
    content_type: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let stream = UnixStream::connect(sock).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Full<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let body_bytes = body.map(<[u8]>::to_vec).unwrap_or_default();
    let mut builder = Request::builder()
        .method(method)
        .uri(format!("http://stt.local{path}"))
        .header("host", "stt.local");
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }
    let req = builder
        .body(Full::new(Bytes::from(body_bytes)))
        .expect("build req");

    let resp = sender.send_request(req).await.expect("send req");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes()
        .to_vec();
    (status, bytes)
}

fn as_json(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).unwrap_or(serde_json::Value::Null)
}

/// An unknown path returns `404`. Unmatched routes never reach the scope
/// middleware (it's layered only on the known route groups), so this holds
/// regardless of the token.
#[tokio::test]
async fn unknown_route_is_not_found() {
    let (_guard, sock) = start_daemon().await;
    let token = http_client::auth_request(sock.clone(), "transport", &["status"])
        .await
        .expect("auth")
        .session_token;

    let (status, _) = send(
        &sock,
        Method::GET,
        "/v1/definitely-not-a-route",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "unknown route must be 404");

    // Also outside the /v1 namespace entirely.
    let (status, _) = send(&sock, Method::GET, "/nope", None, None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A known path hit with the wrong method returns `405`. `/auth/request`
/// is POST-only and sits outside the auth middleware, so a GET reaches the
/// method router directly — no token needed.
#[tokio::test]
async fn wrong_method_on_unauthed_route_is_405() {
    let (_guard, sock) = start_daemon().await;
    let (status, _) = send(&sock, Method::GET, "/v1/auth/request", None, None, None).await;
    assert_eq!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "GET on the POST-only /auth/request must be 405"
    );
}

/// Wrong method also yields `405` on a route guarded by the auth
/// middleware, provided the token is valid: the middleware passes, then the
/// method router rejects. `/ping` is GET-only.
#[tokio::test]
async fn wrong_method_behind_middleware_is_405() {
    let (_guard, sock) = start_daemon().await;
    let token = http_client::auth_request(sock.clone(), "transport", &["status"])
        .await
        .expect("auth")
        .session_token;

    let (status, _) = send(&sock, Method::POST, "/v1/ping", Some(&token), None, None).await;
    assert_eq!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "POST on the GET-only /ping (with a valid token) must be 405"
    );
}

/// A *malformed* JSON body (declared `application/json` but unparseable) is
/// rejected with `400` by the `Json` extractor before the handler runs.
/// (axum 0.8's `Option<Json<T>>` only maps an *absent* body to `None`; a
/// present-but-broken body is a hard rejection — see the next test for the
/// absent-body path.)
#[tokio::test]
async fn malformed_json_body_is_400() {
    let (_guard, sock) = start_daemon().await;

    let (status, _) = send(
        &sock,
        Method::POST,
        "/v1/auth/request",
        None,
        Some(b"{ this is not json "),
        Some("application/json"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a malformed JSON body must be rejected with 400"
    );
}

/// An *absent* body on `/auth/request` is the handler's own
/// `400 auth_denied (invalid_body)` path: `Option<Json<..>>` yields `None`
/// and the handler maps that to the documented invalid-body response.
#[tokio::test]
async fn missing_body_is_invalid_body() {
    let (_guard, sock) = start_daemon().await;

    let (status, body) = send(&sock, Method::POST, "/v1/auth/request", None, None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "missing body must be 400");
    let json = as_json(&body);
    assert_eq!(json["message"], "auth_denied", "got: {json}");
    assert_eq!(json["data"]["reason"], "invalid_body", "got: {json}");
}

/// A settings endpoint uses a *mandatory* `Json<T>` extractor, so a
/// malformed body is rejected with `400` by the extractor before the
/// handler runs. (Auth middleware passes first — a valid settings token is
/// required to reach the extractor.)
#[tokio::test]
async fn malformed_body_on_json_extractor_endpoint_is_400() {
    let (_guard, sock) = start_daemon().await;
    let token = http_client::auth_request(sock.clone(), "transport", &["settings"])
        .await
        .expect("auth settings")
        .session_token;

    let (status, _) = send(
        &sock,
        Method::POST,
        "/v1/audio_theme",
        Some(&token),
        Some(b"}{ not json"),
        Some("application/json"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the Json extractor must reject a malformed body with 400"
    );
}
