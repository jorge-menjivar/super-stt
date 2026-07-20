// SPDX-License-Identifier: GPL-3.0-only
//! A daemon error must reach the caller as that error.
//!
//! The typed transport helpers deserialize a response body into the endpoint's
//! success type. That is only valid for a 2xx: every daemon failure answers
//! with a `{"status":"error", …}` envelope which shares no fields with any
//! success type, so feeding one to the success deserializer reports whichever
//! success field was missing and buries the real cause. A rate-limited backend
//! uninstall surfaced in the app as
//! ``Uninstall failed: Failed to parse response: missing field `uninstalled` at
//! line 1 column 43`` — "column 43" being the length of the 429 body, the only
//! trace of what actually went wrong.
//!
//! These tests drive the real socket path rather than the status-to-message
//! helper alone, because the defect was the missing status check in
//! `send_request`, not the mapping.

use serde::Deserialize;
use super_stt_shared::daemon::http_client::transport;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Stands in for any endpoint's success type — the point is that none of its
/// fields appear in an error envelope.
#[derive(Debug, Deserialize)]
struct UninstallResponse {
    #[allow(dead_code)]
    uninstalled: bool,
    #[allow(dead_code)]
    was_active: bool,
}

/// Serve exactly one request on `socket`, answering with `status` and `body`.
/// Raw bytes rather than a server framework: the response shape under test is
/// a fixed handful of bytes, and the daemon writes its own rejections the same
/// way. Must be called from within a tokio runtime — `bind` registers with the
/// reactor.
fn serve_once(socket: &std::path::Path, status: &'static str, body: &'static str) {
    let listener = tokio::net::UnixListener::bind(socket).expect("bind test socket");
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        // Read the request head so the client's write completes; the body is
        // irrelevant to what we answer.
        let mut buf = [0u8; 2048];
        let _ = stream.read(&mut buf).await;
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    });
}

fn socket_path(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("super-stt-transport-test-{name}.sock"));
    let _ = std::fs::remove_file(&path);
    path
}

/// The exact regression: 429 `rate_limited`, the body that produced
/// "missing field `uninstalled` at line 1 column 43".
#[tokio::test]
async fn rate_limited_uninstall_surfaces_rate_limited() {
    let socket = socket_path("rate-limited");
    serve_once(
        &socket,
        "429 Too Many Requests",
        r#"{"status":"error","message":"rate_limited"}"#,
    );

    let err = transport::delete_json::<UninstallResponse>(socket.clone(), "token", "/backends/x")
        .await
        .expect_err("a 429 is not a successful uninstall");

    let message = err.to_string();
    assert_eq!(message, "rate_limited (HTTP 429)");
    assert!(
        !message.contains("uninstalled"),
        "the success schema must not be blamed for a daemon error: {message}"
    );
    let _ = std::fs::remove_file(&socket);
}

/// The same guard has to hold for the operational failures an uninstall can
/// legitimately hit, so none of them regress into schema complaints.
#[tokio::test]
async fn registry_failures_surface_their_error_code() {
    let socket = socket_path("not-found");
    serve_once(
        &socket,
        "404 Not Found",
        r#"{"status":"error","error_code":"not_found","error":"not_found"}"#,
    );

    let err = transport::delete_json::<UninstallResponse>(socket.clone(), "token", "/backends/x")
        .await
        .expect_err("a 404 is not a successful uninstall");

    assert_eq!(err.to_string(), "not_found (HTTP 404)");
    let _ = std::fs::remove_file(&socket);
}

/// A 401 still takes the typed `InvalidSession` path — the status check added
/// for other failures must not swallow the one callers re-authenticate on.
#[tokio::test]
async fn unauthorized_still_maps_to_invalid_session() {
    let socket = socket_path("unauthorized");
    serve_once(
        &socket,
        "401 Unauthorized",
        r#"{"status":"error","message":"invalid_session","data":{"reason":"expired"}}"#,
    );

    let err = transport::delete_json::<UninstallResponse>(socket.clone(), "token", "/backends/x")
        .await
        .expect_err("a 401 is not a successful uninstall");

    assert!(
        err.is_invalid_session(),
        "401 must stay routable to re-auth, got: {err}"
    );
    assert_eq!(err.to_string(), "invalid_session (expired)");
    let _ = std::fs::remove_file(&socket);
}

/// The streaming endpoints answer a rejected request with the same JSON
/// envelope rather than an event stream. Both used to derive their own error
/// text; they now share the one mapping, and this pins the wiring so neither
/// can quietly drift back.
#[tokio::test]
async fn streaming_endpoints_share_the_same_mapping() {
    let socket = socket_path("events-rejected");
    serve_once(
        &socket,
        "429 Too Many Requests",
        r#"{"status":"error","message":"rate_limited"}"#,
    );
    let err = super_stt_shared::daemon::http_client::events_stream(socket.clone(), "token", &["a"])
        .await
        .err()
        .expect("a 429 is not a subscription");
    assert_eq!(err.to_string(), "rate_limited (HTTP 429)");
    let _ = std::fs::remove_file(&socket);

    let socket = socket_path("transcribe-rejected");
    serve_once(
        &socket,
        "409 Conflict",
        r#"{"status":"error","message":"recording_in_progress"}"#,
    );
    let err = super_stt_shared::daemon::http_client::transcribe_stream(
        socket.clone(),
        "token",
        super_stt_shared::daemon::http_client::TranscribeOptions::default(),
    )
    .await
    .err()
    .expect("a 409 is not a recording");
    assert_eq!(err.to_string(), "recording_in_progress (HTTP 409)");
    let _ = std::fs::remove_file(&socket);
}

/// The success path is untouched: a 2xx body still deserializes into `T`.
#[tokio::test]
async fn successful_uninstall_still_parses() {
    let socket = socket_path("success");
    serve_once(
        &socket,
        "200 OK",
        r#"{"uninstalled":true,"was_active":false}"#,
    );

    let resp = transport::delete_json::<UninstallResponse>(socket.clone(), "token", "/backends/x")
        .await
        .expect("a 200 with a well-formed body is a successful uninstall");

    assert!(resp.uninstalled);
    assert!(!resp.was_active);
    let _ = std::fs::remove_file(&socket);
}
