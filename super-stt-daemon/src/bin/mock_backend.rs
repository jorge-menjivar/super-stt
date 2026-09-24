// SPDX-License-Identifier: GPL-3.0-only
//! Mock subprocess backend for `tests/subprocess_mock.rs`. Serves the `/v1`
//! contract over `SUPER_STT_BACKEND_SOCKET` with canned responses — loads no
//! model and needs no GPU or network. Built only with the `test-fixtures`
//! feature.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
    /// Status polls left before a [`BUILDS_KERNELS`] load is ready; zero when
    /// no such load is under way.
    kernel_polls_left: AtomicU32,
}

#[tokio::main]
async fn main() {
    let socket = std::env::var("SUPER_STT_BACKEND_SOCKET").expect("SUPER_STT_BACKEND_SOCKET");
    let state = Arc::new(AppState {
        loaded: AtomicBool::new(false),
        kernel_polls_left: AtomicU32::new(0),
    });
    let app = Router::new()
        .route(
            "/v1/ping",
            get(|| async { Json(json!({ "status": "success", "message": "pong" })) }),
        )
        .route("/v1/status", get(status))
        .route("/v1/load", post(load))
        .route("/v1/transcribe", post(transcribe))
        .route("/v1/process", post(process))
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

/// Polls a [`BUILDS_KERNELS`] load reports progress for before it is ready.
const KERNEL_POLLS: u32 = 4;

async fn status(State(s): State<Arc<AppState>>) -> Json<Value> {
    // A first load building its kernels: a quarter further on each poll, the
    // way a backend reports its tuning results, then ready.
    let left = s.kernel_polls_left.load(Ordering::SeqCst);
    if left > 0 {
        s.kernel_polls_left.store(left - 1, Ordering::SeqCst);
        if left == 1 {
            s.loaded.store(true, Ordering::SeqCst);
        } else {
            let done = KERNEL_POLLS - left;
            return Json(json!({
                "status": "success",
                "state": "loading",
                "phase": "initial_setup",
                "step": "building_kernels",
                "progress": f64::from(done) / f64::from(KERNEL_POLLS),
            }));
        }
    }
    if s.loaded.load(Ordering::SeqCst) {
        Json(json!({
            "status": "success",
            "state": "ready",
            "device": "cpu",
            "model": { "name": "mock" }
        }))
    } else {
        Json(json!({ "status": "success", "state": "starting" }))
    }
}

/// The model a test loads to have the backend die partway through its load.
const DIES_ON_LOAD: &str = "mock-dies-on-load";

/// The model a test loads to have the backend report building its kernels.
const BUILDS_KERNELS: &str = "mock-builds-kernels";

async fn load(State(s): State<Arc<AppState>>, body: String) -> impl IntoResponse {
    let name = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_string));
    if name.as_deref() == Some(BUILDS_KERNELS) {
        s.kernel_polls_left.store(KERNEL_POLLS, Ordering::SeqCst);
    } else if name.as_deref() == Some(DIES_ON_LOAD) {
        // Accept the load, say why it is failing, and exit: the daemon's poll
        // then finds nobody answering, and the line below is what it should
        // hand back. Exiting after a moment, so the 202 reaches the daemon.
        eprintln!("mock: the model thread panicked while loading");
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            std::process::exit(101);
        });
    } else {
        s.loaded.store(true, Ordering::SeqCst);
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({ "status": "success", "message": "Loading started" })),
    )
}

async fn transcribe(State(s): State<Arc<AppState>>, _body: String) -> (StatusCode, Json<Value>) {
    if !s.loaded.load(Ordering::SeqCst) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "status": "error", "message": "not_ready" })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({ "status": "success", "transcription": "mock transcription" })),
    )
}

/// `POST /v1/process` — echoes the submitted text back prefixed, behind the
/// same `loaded` gate as `transcribe`, so a test can assert both that the route
/// is reached and that the text round-tripped. Any `x-stt-option-*` headers
/// on the request are echoed too, sorted, as ` [name=value …]`, so a test can
/// assert the daemon injected them.
async fn process(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> (StatusCode, Json<Value>) {
    if !s.loaded.load(Ordering::SeqCst) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "status": "error", "message": "not_ready" })),
        );
    }
    let text = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_default();
    if text.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "status": "error", "message": "invalid_text" })),
        );
    }
    let mut options: Vec<String> = headers
        .iter()
        .filter_map(|(k, v)| {
            let name = k.as_str().strip_prefix("x-stt-option-")?;
            Some(format!("{name}={}", v.to_str().unwrap_or("?")))
        })
        .collect();
    options.sort();
    let echoed = if options.is_empty() {
        String::new()
    } else {
        format!(" [{}]", options.join(" "))
    };
    (
        StatusCode::OK,
        Json(json!({ "status": "success", "text": format!("processed: {text}{echoed}") })),
    )
}
