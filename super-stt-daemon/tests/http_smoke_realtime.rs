// SPDX-License-Identifier: GPL-3.0-only
//! Realtime WebSocket transport, end to end over the daemon's real HTTP
//! listener: `GET /v1/transcribe/realtime`.
//!
//! Every other realtime test drives `WasmBackend::realtime_session` directly
//! (`wasm_mock_realtime.rs`), which skips the HTTP layer entirely. That left the
//! protocol upgrade itself untested — and unimplemented: hyper writes the `101
//! Switching Protocols` response but only performs the upgrade when the
//! connection is built with `.with_upgrades()`. Without it the handshake
//! "succeeded" and the socket was dropped before the first frame, so the guest's
//! opening `recv` saw a closed consumer stream and returned without a word. The
//! round-trip test below fails exactly that way if the call is ever lost again.
//!
//! Uses `SUPER_STT_AUTO_APPROVE=1` (no GUI) + `SUPER_STT_KEYRING_MOCK=1`
//! (in-memory keyring), so it runs in the default `cargo test` flow.
#![cfg(feature = "wasm-backends")]

mod common;

use common::{Method, StatusCode, TestDaemon};
use hyper::Request;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1::handshake;
use tokio::net::UnixStream;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const FIXTURE_SOURCE: &str = "github.com/super-stt/mock-realtime";
const REALTIME_MODEL: &str = "mock-realtime-1";
/// Pinned in the fixture component (`MOCK_REALTIME_TRANSCRIPTION`).
const MOCK_TRANSCRIPT: &str = "mock realtime transcription";

/// The prebuilt mock realtime component, or `None` when it isn't built. CI runs
/// `just build-mock-wasm-realtime-backend` first; a bare `cargo test` skips.
fn mock_component() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "tests/fixtures/mock-wasm-realtime-backend/target/wasm32-wasip2/release/mock_wasm_realtime_backend.wasm",
    );
    p.exists().then_some(p)
}

/// Seed a websocket-capable backend serving one `realtime` model, backed by the
/// prebuilt mock component. `supported_devices = ["cpu"]` keeps it clear of the
/// separate online-models gate; realtime is not a property of the device.
fn seed_realtime_backend(data_home: &Path, component: &Path) {
    let backend_dir = data_home
        .join("super-stt")
        .join("backends")
        .join("mock-realtime");
    std::fs::create_dir_all(&backend_dir).expect("create fixture backend dir");

    let toml = format!(
        r#"[backend]
source = "{FIXTURE_SOURCE}"
name = "Mock Realtime"
version = "1.0.0"
kind = "wasm"
entrypoint = "mock.wasm"
contract = "v1"
description = "Realtime transport fixture."
license = "GPL-3.0-only"

[capabilities]
websocket = true

[[models]]
name = "{REALTIME_MODEL}"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]
realtime = true
"#
    );
    std::fs::write(backend_dir.join("backend.toml"), toml).expect("write fixture backend.toml");
    std::fs::copy(component, backend_dir.join("mock.wasm")).expect("stage mock component");
}

async fn start_daemon(scopes: &[&str], component: Option<&Path>) -> (TestDaemon, PathBuf, String) {
    let daemon = common::daemon("realtime");
    if let Some(component) = component {
        seed_realtime_backend(&daemon.home().data, component);
    }
    let daemon = daemon.start().await;
    let token = daemon.token("realtime-smoke", scopes).await;
    let socket = daemon.socket().to_path_buf();
    (daemon, socket, token)
}

/// Issue one HTTP request and return `(status, raw body)`.
async fn send(
    socket_path: &PathBuf,
    method: Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, Vec<u8>) {
    let stream = UnixStream::connect(socket_path).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Full<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let payload = body
        .map(|b| serde_json::to_vec(&b).expect("serialize body"))
        .unwrap_or_default();
    let mut builder = Request::builder()
        .method(method)
        .uri(format!("http://stt.local{path}"))
        .header("host", "stt.local")
        .header("authorization", format!("Bearer {token}"));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Full::new(Bytes::from(payload)))
        .expect("build request");
    let response = sender.send_request(request).await.expect("send request");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, bytes.to_vec())
}

/// Select the realtime model into stage 1 and wait for it to be loaded.
async fn select_realtime_model(socket_path: &PathBuf, token: &str) {
    let (status, body) = send(
        socket_path,
        Method::POST,
        "/v1/pipeline/1/model",
        token,
        Some(serde_json::json!({ "model": REALTIME_MODEL, "source": FIXTURE_SOURCE })),
    )
    .await;
    assert!(
        status.is_success(),
        "selecting the realtime model failed: {status} {}",
        String::from_utf8_lossy(&body)
    );

    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let (status, body) = send(socket_path, Method::GET, "/v1/status", token, None).await;
        if status.is_success() {
            let json: serde_json::Value = serde_json::from_slice(&body).expect("status json");
            if json["model_loaded"] == true && json["current_model"] == REALTIME_MODEL {
                return;
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("realtime model was not loaded within 30s");
}

/// Open the consumer realtime WebSocket over the daemon's Unix socket.
async fn open_realtime_ws(
    socket_path: &PathBuf,
    token: &str,
) -> tokio_tungstenite::WebSocketStream<UnixStream> {
    let mut request = "ws://stt.local/v1/transcribe/realtime"
        .into_client_request()
        .expect("build ws request");
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("header value"),
    );
    let stream = UnixStream::connect(socket_path).await.expect("connect");
    let (ws, _response) = tokio_tungstenite::client_async(request, stream)
        .await
        .expect("websocket upgrade");
    ws
}

/// The whole consumer contract over the real listener: upgrade, `start`, PCM,
/// `stop`, and the `preview` + `done` frames coming back.
///
/// This is the regression guard for the protocol upgrade. Without
/// `.with_upgrades()` on the hyper connection the handshake still returns `101`
/// and this test fails at the first `next()` — the daemon drops the socket
/// before a single frame moves.
#[tokio::test]
async fn realtime_websocket_session_round_trip() {
    let Some(component) = mock_component() else {
        eprintln!(
            "skipping: mock realtime component not built (run `just build-mock-wasm-realtime-backend`)"
        );
        return;
    };
    let (_guard, sock, token) =
        start_daemon(&["settings", "status", "transcribe"], Some(&component)).await;
    select_realtime_model(&sock, &token).await;

    let mut ws = open_realtime_ws(&sock, &token).await;
    ws.send(Message::Text(
        r#"{"type":"start","sample_rate":16000}"#.into(),
    ))
    .await
    .expect("send start");
    // 100 ms of silence, s16le mono — the mock ignores audio, but a real
    // consumer always sends some and the relay must carry it.
    ws.send(Message::Binary(vec![0u8; 3200].into()))
        .await
        .expect("send audio");
    ws.send(Message::Text(r#"{"type":"stop"}"#.into()))
        .await
        .expect("send stop");

    let mut preview = None;
    let mut done = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while preview.is_none() || done.is_none() {
        let next = tokio::time::timeout_at(deadline, ws.next()).await;
        let Ok(frame) = next else {
            panic!("timed out waiting for frames (preview={preview:?}, done={done:?})");
        };
        match frame {
            Some(Ok(Message::Text(text))) => {
                let event: serde_json::Value =
                    serde_json::from_str(&text).expect("frame must be JSON");
                match event["type"].as_str() {
                    Some("preview") => preview = Some(event["text"].as_str().unwrap().to_string()),
                    Some("done") => {
                        done = Some(event["transcription"].as_str().unwrap().to_string());
                    }
                    Some("error") => panic!("backend reported an error: {text}"),
                    _ => {}
                }
            }
            Some(Ok(_)) => {}
            // A close (or a dropped socket) before both frames is the failure
            // the missing upgrade produced.
            Some(Err(e)) => panic!("websocket errored early: {e} (preview={preview:?})"),
            None => panic!("socket closed before done (preview={preview:?})"),
        }
    }

    assert_eq!(preview.unwrap(), MOCK_TRANSCRIPT);
    assert_eq!(done.unwrap(), MOCK_TRANSCRIPT);
}

/// The route sits behind the `transcribe` scope, and the check runs before any
/// upgrade: a `status`-only token is refused outright. Hermetic — no component
/// needed, so this half of the contract is covered even without the wasm build.
#[tokio::test]
async fn realtime_websocket_requires_the_transcribe_scope() {
    let (_guard, sock, token) = start_daemon(&["status"], None).await;

    let (status, _) = send(&sock, Method::GET, "/v1/transcribe/realtime", &token, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a token without `transcribe` must not reach the upgrade"
    );
}
