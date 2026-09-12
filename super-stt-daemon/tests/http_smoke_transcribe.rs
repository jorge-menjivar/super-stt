// SPDX-License-Identifier: GPL-3.0-only
//! `POST /v1/transcribe` with audio the caller already has.
//!
//! The endpoint has four use cases dispatched on the body, and three of them
//! need a microphone. This one does not: a top-level `audio_data` array means
//! "transcribe this buffer", and the daemon must not touch the mic. That makes
//! it the one transcription path a hermetic test can drive end to end — through
//! the real backend host, into a component that answers, and back out as JSON.
//!
//! What it needs is a backend that actually runs, so this stages the prebuilt
//! mock component (`just build-mock-wasm-backend`) rather than the manifest-only
//! fixtures the other smoke tests use, selects it, loads it, and then asks it to
//! transcribe. Without the component built the test says so and returns, the
//! same as the other mock-backed tests.

mod common;

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

/// The fixture backend's `source`, which is how a stage selects it.
const FIXTURE_SOURCE: &str = "github.com/super-stt/mock";

/// The model the fixture serves at stage 1.
const FIXTURE_MODEL: &str = "echo-stt";

/// One transcription model, backed by the mock component. `contract = "v2"` is
/// what the component speaks; `none` is not offered because a stage that loads
/// needs a device it can resolve.
const FIXTURE_MANIFEST: &str = r#"[backend]
source = "github.com/super-stt/mock"
name = "Mock STT"
version = "1.0.0"
kind = "wasm"
entrypoint = "mock.wasm"
contract = "v2"
id = "app.super-stt.mock"
description = "A backend that answers, so a transcript can come back."
license = "GPL-3.0-only"

[network]
allowed_hosts = []

[[models]]
name = "echo-stt"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]
"#;

struct DaemonGuard {
    child: Child,
    cleanup_paths: Vec<PathBuf>,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        common::shutdown(&mut self.child);
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

async fn start_daemon(component: &Path) -> (DaemonGuard, PathBuf, String) {
    let unique = format!("stt-transcribe-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    let data_home = tmp.join(format!("{unique}-data"));
    let cache_home = tmp.join(format!("{unique}-cache"));
    for d in [&config_home, &data_home, &cache_home] {
        std::fs::create_dir_all(d).expect("create test xdg dir");
    }

    common::BackendFixture {
        dir_name: "mock-stt",
        manifest: FIXTURE_MANIFEST,
        entrypoint: "mock.wasm",
        // Real bytes: this backend is asked to answer, not just to exist.
        component: Some(component),
    }
    .install(&data_home);

    let child = Command::new(DAEMON_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1")
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", &http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_CACHE_HOME", &cache_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    let guard = DaemonGuard {
        child,
        cleanup_paths: vec![http_socket.clone(), config_home, data_home, cache_home],
    };

    let deadline = Instant::now() + Duration::from_mins(2);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && let Ok(auth) = http_client::auth_request(
                http_socket.clone(),
                "transcribe-smoke",
                &["settings", "transcribe"],
            )
            .await
        {
            return (guard, http_socket, auth.session_token);
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("daemon HTTP listener not ready within 120s");
}

async fn raw_request(
    socket_path: &PathBuf,
    method: Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let stream = UnixStream::connect(socket_path).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Full<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let body_bytes = body
        .map(|b| serde_json::to_vec(&b).expect("encode body"))
        .unwrap_or_default();
    let mut builder = Request::builder()
        .method(method)
        .uri(format!("http://stt.local/v1{path}"))
        .header("host", "stt.local")
        .header("authorization", format!("Bearer {token}"));
    if !body_bytes.is_empty() {
        builder = builder
            .header("content-type", "application/json")
            .header("content-length", body_bytes.len().to_string());
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
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

async fn get(p: &PathBuf, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
    raw_request(p, Method::GET, path, token, None).await
}

async fn post(
    p: &PathBuf,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    raw_request(p, Method::POST, path, token, Some(body)).await
}

/// Select the fixture backend at stage 1 and load its model, then wait for the
/// stage to report it running. The load is asynchronous — the endpoint answers
/// before it finishes — so this polls the way `http_smoke_stage_flow` does.
async fn load_stage_one(sock: &PathBuf, token: &str) {
    let (status, body) = post(
        sock,
        "/pipeline/1",
        token,
        serde_json::json!({ "source": FIXTURE_SOURCE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "select backend: {body}");

    let (status, body) = post(
        sock,
        "/pipeline/1/model",
        token,
        serde_json::json!({ "model": FIXTURE_MODEL }),
    )
    .await;
    assert!(status.is_success(), "load model answered {status}: {body}");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = serde_json::Value::Null;
    while Instant::now() < deadline {
        let (_, body) = get(sock, "/pipeline/1/model", token).await;
        last = body["model"].clone();
        if last["loaded"] == true {
            return;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("stage 1 never came up within 30s; last slot: {last}");
}

/// A quarter-second of silence at 16 kHz. The mock answers with a fixed
/// transcript whatever it is handed, so the samples only have to be a plausible
/// buffer — but they do have to survive the round trip as `f32`.
fn silence() -> Vec<f32> {
    vec![0.0; 4000]
}

/// The whole point: hand the daemon a buffer and get a transcript back, with no
/// microphone anywhere in it.
///
/// This is the one path through `/transcribe` that runs the real backend host
/// end to end — request built, dispatched to a loaded component, and narrowed
/// back to the documented `{status, transcription}` body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn precaptured_audio_comes_back_as_a_transcript() {
    let Some(component) = common::mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };
    let (_guard, sock, token) = start_daemon(&component).await;
    load_stage_one(&sock, &token).await;

    let (status, body) = post(
        &sock,
        "/transcribe",
        &token,
        serde_json::json!({ "audio_data": silence(), "sample_rate": 16000 }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "POST /transcribe: {body}");
    assert_eq!(body["status"], "success", "{body}");
    assert_eq!(
        body["transcription"],
        common::MOCK_TRANSCRIPTION,
        "the transcript must come from the component that was loaded: {body}"
    );
}

/// A per-request `language` rides along with the buffer. The mock ignores it,
/// so what this pins is that naming a language does not make the request
/// invalid — the override is read off the top level and carried, not rejected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn precaptured_audio_accepts_a_language_override() {
    let Some(component) = common::mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };
    let (_guard, sock, token) = start_daemon(&component).await;
    load_stage_one(&sock, &token).await;

    let (status, body) = post(
        &sock,
        "/transcribe",
        &token,
        serde_json::json!({ "audio_data": silence(), "sample_rate": 16000, "language": "en" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "POST /transcribe with language: {body}"
    );
    assert_eq!(body["transcription"], common::MOCK_TRANSCRIPTION, "{body}");
}

/// `stream_realtime` asks for incremental frames off the microphone, and
/// `audio_data` says there is no microphone in this request. The two cannot both
/// be honored, so the daemon refuses rather than silently dropping one — a
/// client that sent both is confused about which call it is making.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn precaptured_audio_refuses_a_realtime_stream() {
    let Some(component) = common::mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };
    let (_guard, sock, token) = start_daemon(&component).await;

    let (status, body) = post(
        &sock,
        "/transcribe",
        &token,
        serde_json::json!({ "audio_data": silence(), "stream_realtime": true }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["status"], "error", "{body}");
    assert_eq!(
        body["message"], "stream_realtime_with_audio_data",
        "the refusal must name which pair of fields conflicted: {body}"
    );
}

/// `audio_data` that is not a list of numbers is a `400`, not a panic and — the
/// part that matters — not a fallback to opening the microphone. A malformed
/// buffer must fail as a bad request, never as a silent recording.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audio_data_that_is_not_samples_is_rejected() {
    let Some(component) = common::mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };
    let (_guard, sock, token) = start_daemon(&component).await;

    let (status, body) = post(
        &sock,
        "/transcribe",
        &token,
        serde_json::json!({ "audio_data": ["not", "numbers"] }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["status"], "error", "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("audio_data")),
        "the message must say which field was bad: {body}"
    );
}

/// Pre-captured audio with nothing loaded to transcribe it. The daemon has to
/// answer with the error the command bus produced rather than a `200` carrying
/// an empty transcript — an empty string is a legitimate transcription of
/// silence, so a client cannot tell the two apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn precaptured_audio_with_no_model_loaded_is_an_error() {
    let Some(component) = common::mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };
    // Deliberately no `load_stage_one`: the backend is installed but nothing
    // has been selected or loaded.
    let (_guard, sock, token) = start_daemon(&component).await;

    let (status, body) = post(
        &sock,
        "/transcribe",
        &token,
        serde_json::json!({ "audio_data": silence(), "sample_rate": 16000 }),
    )
    .await;

    assert!(
        status.is_client_error() || status.is_server_error(),
        "no model loaded must not answer 2xx, got {status}: {body}"
    );
    assert_eq!(body["status"], "error", "{body}");
}
