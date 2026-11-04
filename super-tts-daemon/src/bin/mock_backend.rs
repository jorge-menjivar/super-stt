// SPDX-License-Identifier: GPL-3.0-only
//! Mock subprocess backend for `tests/subprocess_mock.rs`. Serves the `/v1`
//! contract over `SUPER_TTS_BACKEND_SOCKET` with canned responses — loads no
//! model and needs no GPU or network. Built only with the `test-fixtures`
//! feature.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use serde_json::{Value, json};
use tokio::net::UnixListener;

struct AppState {
    loaded: AtomicBool,
    /// A prepare finished and no load has started since.
    prepared: AtomicBool,
}

#[tokio::main]
async fn main() {
    let socket = std::env::var("SUPER_TTS_BACKEND_SOCKET").expect("SUPER_TTS_BACKEND_SOCKET");
    let state = Arc::new(AppState {
        loaded: AtomicBool::new(false),
        prepared: AtomicBool::new(false),
    });
    let app = Router::new()
        .route(
            "/v1/ping",
            get(|| async { Json(json!({ "status": "success", "message": "pong" })) }),
        )
        .route("/v1/status", get(status))
        .route("/v1/load", post(load))
        .route("/v1/prepare", post(prepare))
        .route("/v1/synthesize", post(synthesize))
        .route(
            "/v1/cancel",
            post(|| async { Json(json!({ "status": "success", "message": "Cancelled" })) }),
        )
        .layer(DefaultBodyLimit::disable())
        .with_state(state);

    if let Some(parent) = std::path::Path::new(&socket).parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).expect("bind socket");

    loop {
        let (stream, _) = listener.accept().await.expect("accept");
        let app = app.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let svc = TowerToHyperService::new(app);
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, svc)
                .await;
        });
    }
}

async fn status(State(s): State<Arc<AppState>>) -> Json<Value> {
    if s.loaded.load(Ordering::SeqCst) {
        Json(json!({
            "status": "success",
            "state": "ready",
            "device": "cpu",
            "model": { "name": "mock" }
        }))
    } else if s.prepared.load(Ordering::SeqCst) {
        Json(json!({ "status": "success", "state": "prepared" }))
    } else {
        Json(json!({ "status": "success", "state": "starting" }))
    }
}

/// The model a test prepares to have the prepare fail, which must leave its
/// load unharmed.
const PREPARE_FAILS: &str = "mock-prepare-fails";

/// The model a test prepares to have the backend exit during the prepare, so
/// the daemon has to start it again before the load.
const DIES_ON_PREPARE: &str = "mock-dies-on-prepare";

/// The file a successful prepare leaves in the cache directory: the files the
/// backend directory held when the prepare arrived, one per line, sorted. The
/// daemon does not poll a prepare, so this is how a test sees that one ran
/// and what was on disk for it.
const PREPARED_MARKER: &str = "prepared";

/// `POST /v1/prepare` — record what was on disk, unless the model is one that
/// makes the prepare fail or the backend exit.
async fn prepare(State(s): State<Arc<AppState>>, body: String) -> impl IntoResponse {
    if s.loaded.load(Ordering::SeqCst) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "status": "error", "message": "already_loading" })),
        );
    }
    let name = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_string));
    match name.as_deref() {
        // A failed prepare is only logged, and the state stays `starting`.
        Some(PREPARE_FAILS) => eprintln!("mock: the prepare failed"),
        Some(DIES_ON_PREPARE) => {
            eprintln!("mock: the driver crashed while preparing");
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                std::process::exit(139);
            });
        }
        _ => {
            record_prepare();
            s.prepared.store(true, Ordering::SeqCst);
        }
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({ "status": "success", "message": "Preparing started" })),
    )
}

/// Write [`PREPARED_MARKER`] into the cache directory.
fn record_prepare() {
    let (Ok(dir), Ok(cache)) = (
        std::env::var("SUPER_TTS_BACKEND_DIR"),
        std::env::var("SUPER_TTS_BACKEND_CACHE_DIR"),
    ) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let mut files = Vec::new();
    let mut pending = vec![dir.clone()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(relative) = path.strip_prefix(&dir) {
                files.push(relative.display().to_string());
            }
        }
    }
    files.sort();
    let _ = std::fs::write(
        std::path::Path::new(&cache).join(PREPARED_MARKER),
        files.join("\n"),
    );
}

async fn load(State(s): State<Arc<AppState>>, _body: String) -> impl IntoResponse {
    s.prepared.store(false, Ordering::SeqCst);
    s.loaded.store(true, Ordering::SeqCst);
    (
        StatusCode::ACCEPTED,
        Json(json!({ "status": "success", "message": "Loading started" })),
    )
}

/// Sample rate the mock declares on `/v1/synthesize`.
pub const MOCK_SAMPLE_RATE: u32 = 24000;
/// Samples the mock synthesizes, split across [`MOCK_AUDIO_FRAMES`] frames so
/// the test exercises reassembly rather than a single-frame happy path.
pub const MOCK_SAMPLE_COUNT: usize = 960;
/// How many audio frames the ramp is split across.
pub const MOCK_AUDIO_FRAMES: usize = 4;

/// The known ramp the mock emits: sample `i` is `i` as an `i16`. A ramp rather
/// than silence, so a decoder that drops or misaligns bytes yields a visibly
/// wrong sequence instead of a plausible one.
#[must_use]
pub fn mock_samples() -> Vec<i16> {
    (0..MOCK_SAMPLE_COUNT)
        .map(|i| i16::try_from(i).unwrap_or(i16::MAX))
        .collect()
}

/// `POST /v1/synthesize` — the framed binary body: several `audio` frames
/// carrying the ramp as s16le, one `mark`, then `done`.
///
/// Frames are built by hand rather than with the shared codec: this fixture
/// stands in for a third-party backend, so encoding it independently means the
/// test checks the wire format instead of round-tripping one implementation
/// against itself.
const KIND_AUDIO: u8 = 0x01;
const KIND_MARK: u8 = 0x02;
const KIND_DONE: u8 = 0x03;

/// `[u8 kind][u32 len little-endian][payload]`.
fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(kind);
    out.extend_from_slice(
        &u32::try_from(payload.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(payload);
    out
}

/// `POST /v1/synthesize` — a framed s16le ramp, and the `x-tts-option-*`
/// headers the request carried echoed into the mark frame as `options`.
///
/// The echo rides on the mark because that is the one frame in the stream with
/// a payload a test can read without decoding audio, and `Mark` ignores a field
/// it does not know. It is what lets `option_headers_reach_the_subprocess`
/// assert the daemon injected the user's settings on the *streaming* path,
/// which builds its own request rather than going through `request`.
async fn synthesize(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    _body: String,
) -> axum::response::Response {
    use axum::http::header::CONTENT_TYPE;

    if !s.loaded.load(Ordering::SeqCst) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "status": "error", "message": "not_ready" })),
        )
            .into_response();
    }

    let pcm: Vec<u8> = mock_samples()
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    let mut body = Vec::new();
    for chunk in pcm.chunks(pcm.len() / MOCK_AUDIO_FRAMES) {
        body.extend(frame(KIND_AUDIO, chunk));
    }
    let mut options: Vec<String> = headers
        .iter()
        .filter_map(|(k, v)| {
            let name = k.as_str().strip_prefix("x-tts-option-")?;
            Some(format!("{name}={}", v.to_str().unwrap_or("?")))
        })
        .collect();
    options.sort();
    let mark = format!(
        r#"{{"start_ms":0,"end_ms":40,"start_char":0,"end_char":5,"options":"{}"}}"#,
        options.join(" ")
    );
    body.extend(frame(KIND_MARK, mark.as_bytes()));
    body.extend(frame(KIND_DONE, b"{}"));

    (
        StatusCode::OK,
        [
            (CONTENT_TYPE, "application/vnd.super-tts.frames"),
            (
                axum::http::HeaderName::from_static("x-tts-sample-rate"),
                "24000",
            ),
            (axum::http::HeaderName::from_static("x-tts-channels"), "1"),
            (axum::http::HeaderName::from_static("x-tts-format"), "s16le"),
        ],
        body,
    )
        .into_response()
}
