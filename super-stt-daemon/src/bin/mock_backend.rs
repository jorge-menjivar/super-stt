// SPDX-License-Identifier: GPL-3.0-only
//! Mock subprocess backend for `tests/subprocess_mock.rs`. Serves the `/v1`
//! contract over `SUPER_STT_BACKEND_SOCKET` with canned responses — loads no
//! model and needs no GPU or network. Built only with the `test-fixtures`
//! feature.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use serde_json::{Value, json};
use tokio::net::UnixListener;

struct AppState {
    loaded: AtomicBool,
}

#[tokio::main]
async fn main() {
    let socket = std::env::var("SUPER_STT_BACKEND_SOCKET").expect("SUPER_STT_BACKEND_SOCKET");
    let state = Arc::new(AppState {
        loaded: AtomicBool::new(false),
    });
    let app = Router::new()
        .route(
            "/v1/ping",
            get(|| async { Json(json!({ "status": "success", "message": "pong" })) }),
        )
        .route("/v1/status", get(status))
        .route("/v1/load", post(load))
        .route("/v1/transcribe", post(transcribe))
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
    } else {
        Json(json!({ "status": "success", "state": "starting" }))
    }
}

async fn load(State(s): State<Arc<AppState>>, _body: String) -> impl IntoResponse {
    s.loaded.store(true, Ordering::SeqCst);
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
