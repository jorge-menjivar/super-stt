// SPDX-License-Identifier: GPL-3.0-only
//! `GET /v1/speak/stream` really upgrades, and the upgraded connection is
//! usable.
//!
//! Every other smoke test sends one request and reads one response, which is a
//! shape the daemon can serve while its WebSocket support is entirely broken:
//! hyper writes `101 Switching Protocols` from the router's response like any
//! other, and only afterwards decides whether to hand the IO to
//! `hyper::upgrade::on` or drop it. A test that stopped at the `101` would have
//! passed throughout the period when no frame could cross.
//!
//! So this one goes past the handshake and asserts a frame comes back.
//!
//! Uses `SUPER_TTS_AUTO_APPROVE=1` (no GUI) + `SUPER_TTS_KEYRING_MOCK=1`
//! (in-memory keyring), so it runs in the default `cargo test` flow.

mod common;

use common::TestDaemon;

use futures_util::{SinkExt, StreamExt};
use std::path::PathBuf;
use std::time::Duration;
use super_tts_shared::daemon::http_client;
use tokio::net::UnixStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// Per test, not per process: the tests in this file run in parallel in one
/// process, and each needs a daemon of its own.
async fn start_daemon() -> (TestDaemon, PathBuf) {
    let daemon = common::daemon("speak-stream").start().await;
    let socket = daemon.socket().to_path_buf();
    (daemon, socket)
}

/// A frame crosses the upgraded connection in each direction.
///
/// No model is loaded in a fresh test daemon, so a well-formed `start` is
/// answered with `error: model_not_loaded` — which is the point. The assertion
/// is not *which* frame comes back but that one does at all: reaching the
/// handler means the `101` was followed by a live socket rather than by a
/// hang-up.
///
/// Without `.with_upgrades()` on the server's connection builder, hyper writes
/// the `101` and then drops the IO instead of handing it to
/// `hyper::upgrade::on`. The handshake still completes — `tungstenite` sees a
/// valid `101` — and the first read then fails, which is exactly how this
/// shipped broken without any test noticing.
#[tokio::test]
async fn speak_stream_upgrades_to_a_usable_websocket() {
    let (_guard, socket) = start_daemon().await;

    let auth = http_client::auth_request(socket.clone(), "speak-stream-smoke", &["speak"])
        .await
        .expect("mint a speak-scoped token");

    let stream = UnixStream::connect(&socket)
        .await
        .expect("connect to the daemon socket");

    let mut request = "ws://tts.local/v1/speak/stream"
        .into_client_request()
        .expect("build the upgrade request");
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", auth.session_token)
            .parse()
            .expect("a bearer header"),
    );

    let (mut ws, response) = tokio_tungstenite::client_async(request, stream)
        .await
        .expect("the handshake completes");
    assert_eq!(
        response.status(),
        101,
        "the daemon must answer the upgrade with 101"
    );

    ws.send(Message::Text(r#"{"type":"start"}"#.into()))
        .await
        .expect("send the start frame");

    // A generous bound: this is a liveness assertion, not a latency one. On the
    // broken path the read fails immediately rather than timing out, so the
    // budget costs nothing when the test is going to fail.
    let frame = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("the daemon answers within 10s rather than leaving the socket silent")
        .expect("the stream is still open")
        .expect("the frame is readable");

    let text = frame
        .into_text()
        .expect("the daemon answers with a text frame");
    let parsed: serde_json::Value =
        serde_json::from_str(&text).expect("the answer is a JSON frame");
    assert_eq!(
        parsed["type"], "error",
        "a fresh daemon has no model loaded, so `start` is refused: {text}"
    );
    assert_eq!(
        parsed["message"], "model_not_loaded",
        "the refusal names the reason: {text}"
    );
}

/// The `start` frame carries nothing. A frame still naming one of the removed
/// fields is refused by name rather than having it quietly dropped — serde
/// would ignore the key, and a client whose chosen voice vanished in silence
/// has no way to find out. The refusal cannot be an HTTP status, since `start`
/// arrives after the handshake, so it is a terminal `error` frame.
#[tokio::test]
async fn a_start_frame_naming_a_removed_field_is_refused() {
    let (_guard, socket) = start_daemon().await;

    let auth = http_client::auth_request(socket.clone(), "speak-stream-fields", &["speak"])
        .await
        .expect("mint a speak-scoped token");

    let stream = UnixStream::connect(&socket)
        .await
        .expect("connect to the daemon socket");
    let mut request = "ws://tts.local/v1/speak/stream"
        .into_client_request()
        .expect("build the upgrade request");
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", auth.session_token)
            .parse()
            .expect("a bearer header"),
    );
    let (mut ws, response) = tokio_tungstenite::client_async(request, stream)
        .await
        .expect("the handshake completes");
    assert_eq!(
        response.status(),
        101,
        "the check is on the frame, not the upgrade"
    );

    ws.send(Message::Text(
        r#"{"type":"start","voice":"af_bella"}"#.into(),
    ))
    .await
    .expect("send the start frame");

    let frame = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("the daemon answers within 10s")
        .expect("the stream is still open")
        .expect("the frame is readable");
    let text = frame.into_text().expect("a text frame");
    let parsed: serde_json::Value =
        serde_json::from_str(&text).expect("the answer is a JSON frame");
    assert_eq!(parsed["type"], "error", "naming a voice is refused: {text}");
    let message = parsed["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("voice"),
        "the refusal says which field to drop, rather than blaming the missing model: {text}"
    );
}
