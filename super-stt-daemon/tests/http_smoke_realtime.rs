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

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1::handshake;
use hyper::{Method, Request, StatusCode};
use super_stt_shared::daemon::http_client;
use tokio::net::UnixStream;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-stt-daemon");
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

/// Boot a daemon against a temp socket + XDG dirs. `component` seeds the
/// realtime fixture when given; without it the daemon has no realtime model.
async fn start_daemon(scopes: &[&str], component: Option<&Path>) -> (DaemonGuard, PathBuf, String) {
    let unique = format!("stt-realtime-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    let data_home = tmp.join(format!("{unique}-data"));
    std::fs::create_dir_all(&config_home).expect("create test config dir");
    std::fs::create_dir_all(&data_home).expect("create test data dir");
    if let Some(component) = component {
        seed_realtime_backend(&data_home, component);
    }

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

    // Hand the child to the guard before the readiness loop: the timeout panic
    // below must still kill and reap the daemon, not leak it.
    let guard = DaemonGuard {
        child,
        cleanup_paths: vec![http_socket.clone(), config_home, data_home],
    };

    let deadline = Instant::now() + Duration::from_mins(2);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && http_client::auth_request(http_socket.clone(), "realtime-smoke-probe", &["status"])
                .await
                .is_ok()
        {
            let auth = http_client::auth_request(http_socket.clone(), "realtime-smoke", scopes)
                .await
                .expect("auth_request for test scopes");
            return (guard, http_socket, auth.session_token);
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("daemon HTTP listener not ready within 120s");
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
