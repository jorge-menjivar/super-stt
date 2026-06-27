// SPDX-License-Identifier: GPL-3.0-only
//! HTTP smoke test for the `GET /events` SSE endpoint's **per-topic scope
//! gate** and topic validation.
//!
//! `widget_smoke.rs` covers the subscription *lifecycle* (reconnect /
//! daemon-restart recovery) with a token that already holds every scope it
//! needs. What it does NOT cover is the authorization boundary the handler
//! enforces (`events.rs:66`): each requested topic is gated by a specific
//! scope, and the whole subscription is refused if the token is missing the
//! scope for *any* requested topic. This test pins that boundary plus the
//! `400 invalid_topic` paths:
//!
//! - valid topic + matching scope          → `200` SSE + a `subscribed` ack
//! - topic whose scope the token lacks      → `403 scope_denied`
//! - mixed batch missing one topic's scope  → `403` (all-or-nothing)
//! - unknown topic name                     → `400 invalid_topic { reason: <name> }`
//! - missing / empty `?topics=`             → `400 invalid_topic { reason: missing_topics }`
//! - bogus bearer token                     → `401 invalid_session`
//!
//! Topic→scope mapping under test (`daemon/events.rs::required_scope`):
//! `recording_state` → `recording_events`, `frequency_bands` →
//! `audio_visualization`.
//!
//! Hermetic: `SUPER_STT_AUTO_APPROVE=1` (no GUI) + `SUPER_STT_KEYRING_MOCK=1`
//! (in-memory keyring), so it runs in the default `cargo test` flow.

use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1::handshake;
use hyper::{Method, Request, StatusCode};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use super_stt_shared::daemon::http_client;
use tokio::net::UnixStream;
use tokio::time::{sleep, timeout};

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
    let unique = format!("stt-events-{}-{}", std::process::id(), next_test_uniq());
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
        .arg("--device")
        .arg("cpu")
        .arg("--audio-theme")
        .arg("silent")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && http_client::auth_request(http_socket.clone(), "events-smoke", &["status"])
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

/// Mint a token with the given scopes via the (auto-approved) consent flow.
async fn mint(sock: &PathBuf, scopes: &[&str]) -> String {
    http_client::auth_request(sock.clone(), "events scope smoke", scopes)
        .await
        .expect("auth_request")
        .session_token
}

/// Issue `GET /v1/events?<query>` and return a live HTTP/1 sender plus the
/// response. The connection is driven by a spawned task; the caller must
/// keep the returned `sender` alive until it has finished reading the body
/// (dropping it early can tear down an in-flight streaming response).
async fn open_events(
    sock: &PathBuf,
    query: &str,
    token: &str,
) -> (
    hyper::client::conn::http1::SendRequest<Empty<Bytes>>,
    hyper::Response<hyper::body::Incoming>,
) {
    let stream = UnixStream::connect(sock).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Empty<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("http://stt.local/v1/events?{query}"))
        .header("host", "stt.local")
        .header("authorization", format!("Bearer {token}"))
        .body(Empty::<Bytes>::new())
        .expect("build req");

    let resp = sender.send_request(req).await.expect("send req");
    (sender, resp)
}

/// For the error paths (`4xx`): the response body is a finite JSON document,
/// so it's safe to collect in full. Returns `(status, parsed JSON)`.
async fn events_error(sock: &PathBuf, query: &str, token: &str) -> (StatusCode, serde_json::Value) {
    let (_sender, resp) = open_events(sock, query, token).await;
    let status = resp.status();
    let bytes = timeout(Duration::from_secs(5), resp.into_body().collect())
        .await
        .expect("an error response body must be finite, not an open SSE stream")
        .expect("collect body")
        .to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// For the success path (`200`): the body is an open `text/event-stream`, so
/// we read frames only until the immediate `subscribed` ack appears (or a
/// short timeout), then drop the connection. Returns `(status, content-type,
/// accumulated SSE text)`.
async fn events_subscribe(
    sock: &PathBuf,
    query: &str,
    token: &str,
) -> (StatusCode, String, String) {
    let (_sender, resp) = open_events(sock, query, token).await;
    let status = resp.status();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();

    let mut body = resp.into_body();
    let mut text = String::new();
    let read = async {
        while let Some(frame) = body.frame().await {
            let Ok(frame) = frame else { break };
            if let Some(data) = frame.data_ref() {
                text.push_str(&String::from_utf8_lossy(data));
                // The handler acks the subscription immediately; once we've
                // seen the full event frame we have what we need.
                if text.contains("event: subscribed") && text.contains("\n\n") {
                    break;
                }
            }
        }
    };
    let _ = timeout(Duration::from_secs(5), read).await;
    (status, content_type, text)
}

/// POST `/v1/language` with `{ "language": <tag> }` on a fresh connection and
/// return the status. Used to trigger the daemon's `settings_changed` broadcast.
async fn post_language(sock: &PathBuf, token: &str, tag: &str) -> StatusCode {
    let stream = UnixStream::connect(sock).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Full<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = Request::builder()
        .method(Method::POST)
        .uri("http://stt.local/v1/language")
        .header("host", "stt.local")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(format!(
            "{{\"language\":\"{tag}\"}}"
        ))))
        .expect("build req");
    let resp = sender.send_request(req).await.expect("send req");
    let status = resp.status();
    let _ = resp.into_body().collect().await; // drain so the request fully completes
    status
}

/// Read SSE frames from `body` until `needle` appears in the accumulated text,
/// or the timeout elapses. Returns the accumulated text.
async fn read_until(body: &mut hyper::body::Incoming, needle: &str) -> String {
    let mut text = String::new();
    let read = async {
        while let Some(frame) = body.frame().await {
            let Ok(frame) = frame else { break };
            if let Some(d) = frame.data_ref() {
                text.push_str(&String::from_utf8_lossy(d));
                if text.contains(needle) {
                    break;
                }
            }
        }
    };
    let _ = timeout(Duration::from_secs(5), read).await;
    text
}

/// Changing the global Primary Language broadcasts a `daemon_status_changed`
/// event with `status: "settings_changed"` / `setting: "language"`, so a client
/// showing a per-model language that follows the global value can re-resolve.
/// The handler attaches the broadcast receiver before the `subscribed` ack
/// (events.rs), so subscribing then mutating is race-free.
#[tokio::test]
async fn language_change_broadcasts_settings_changed() {
    let (_guard, sock) = start_daemon().await;
    // `daemon_status` to subscribe to the topic; `settings` to POST the language.
    let token = mint(&sock, &["daemon_status", "settings"]).await;

    // Keep `_sender` alive for the whole test so the stream stays open.
    let (_sender, resp) = open_events(&sock, "topics=daemon_status_changed", &token).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "subscribe to daemon_status_changed should open the stream"
    );
    let mut body = resp.into_body();

    let ack = read_until(&mut body, "event: subscribed").await;
    assert!(ack.contains("event: subscribed"), "missing ack: {ack:?}");

    let st = post_language(&sock, &token, "es-MX").await;
    assert_eq!(st, StatusCode::OK, "POST /v1/language should succeed");

    let text = read_until(&mut body, "settings_changed").await;
    assert!(
        text.contains("event: daemon_status_changed"),
        "settings_changed must ride the daemon_status_changed topic: {text:?}"
    );
    assert!(
        text.contains("\"status\":\"settings_changed\""),
        "payload must carry status=settings_changed: {text:?}"
    );
    assert!(
        text.contains("\"setting\":\"language\""),
        "payload must name the changed setting: {text:?}"
    );
}

/// A valid topic with the matching scope opens the stream and gets an
/// immediate `subscribed` ack listing the topic.
#[tokio::test]
async fn valid_topic_with_scope_subscribes() {
    let (_guard, sock) = start_daemon().await;
    let token = mint(&sock, &["recording_events"]).await;

    let (status, content_type, text) =
        events_subscribe(&sock, "topics=recording_state", &token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "subscribe should open the stream: {text}"
    );
    assert!(
        content_type.starts_with("text/event-stream"),
        "events must be an SSE stream, got content-type `{content_type}`"
    );
    assert!(
        text.contains("event: subscribed"),
        "the stream must open with a `subscribed` ack: {text:?}"
    );
    assert!(
        text.contains("recording_state"),
        "the ack must echo the subscribed topic: {text:?}"
    );
}

/// A token holding all the required scopes can subscribe to a mixed batch;
/// the ack lists every requested topic.
#[tokio::test]
async fn multi_topic_with_all_scopes_subscribes() {
    let (_guard, sock) = start_daemon().await;
    let token = mint(&sock, &["recording_events", "audio_visualization"]).await;

    let (status, _ct, text) =
        events_subscribe(&sock, "topics=recording_state,frequency_bands", &token).await;
    assert_eq!(status, StatusCode::OK, "subscribe should succeed: {text}");
    assert!(text.contains("event: subscribed"), "missing ack: {text:?}");
    assert!(
        text.contains("recording_state"),
        "ack missing recording_state: {text:?}"
    );
    assert!(
        text.contains("frequency_bands"),
        "ack missing frequency_bands: {text:?}"
    );
}

/// Requesting a topic whose granting scope the token lacks is refused with
/// `403 scope_denied` — even though the token IS authenticated (it holds a
/// different, valid scope). This is the per-topic gate, distinct from the
/// middleware's authentication check.
#[tokio::test]
async fn topic_without_its_scope_is_forbidden() {
    let (_guard, sock) = start_daemon().await;
    // Has recording_events but NOT audio_visualization.
    let token = mint(&sock, &["recording_events"]).await;

    let (status, body) = events_error(&sock, "topics=frequency_bands", &token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "frequency_bands needs audio_visualization: {body}"
    );
    assert_eq!(body["message"], "scope_denied", "got: {body}");
}

/// A batch is all-or-nothing: one in-scope topic plus one out-of-scope topic
/// refuses the *whole* subscription rather than silently dropping the
/// ungranted one.
#[tokio::test]
async fn mixed_batch_missing_one_scope_is_forbidden() {
    let (_guard, sock) = start_daemon().await;
    let token = mint(&sock, &["recording_events"]).await;

    let (status, body) =
        events_error(&sock, "topics=recording_state,frequency_bands", &token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the batch must be refused because frequency_bands' scope is missing: {body}"
    );
    assert_eq!(body["message"], "scope_denied");
}

/// An unknown topic name is rejected with `400 invalid_topic`, echoing the
/// offending name in `data.reason`.
#[tokio::test]
async fn unknown_topic_is_invalid_topic() {
    let (_guard, sock) = start_daemon().await;
    // A broad token so the rejection is about the topic, not the scope.
    let token = mint(&sock, &["recording_events", "audio_visualization"]).await;

    let (status, body) = events_error(&sock, "topics=not_a_real_topic", &token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "got: {body}");
    assert_eq!(body["message"], "invalid_topic");
    assert_eq!(
        body["data"]["reason"], "not_a_real_topic",
        "invalid_topic should echo the bad name: {body}"
    );
}

/// Missing or empty `?topics=` is `400 invalid_topic { reason: missing_topics }`.
/// Topic validation runs before the auth-context check, so it triggers even
/// for an otherwise-fine token.
#[tokio::test]
async fn missing_and_empty_topics_are_invalid_topic() {
    let (_guard, sock) = start_daemon().await;
    let token = mint(&sock, &["recording_events"]).await;

    // No `topics` query param at all.
    let (status, body) = events_error(&sock, "", &token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "missing topics: {body}");
    assert_eq!(body["message"], "invalid_topic");
    assert_eq!(body["data"]["reason"], "missing_topics");

    // Present but empty.
    let (status, body) = events_error(&sock, "topics=", &token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "empty topics: {body}");
    assert_eq!(body["data"]["reason"], "missing_topics");
}

/// A bogus bearer token is rejected at the middleware with `401
/// invalid_session` before any topic/scope logic runs.
#[tokio::test]
async fn bogus_token_is_unauthorized() {
    let (_guard, sock) = start_daemon().await;
    let (status, body) = events_error(&sock, "topics=recording_state", "not-a-real-token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "got: {body}");
    assert_eq!(body["message"], "invalid_session");
}
