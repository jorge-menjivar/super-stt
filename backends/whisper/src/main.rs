// SPDX-License-Identifier: GPL-3.0-only
//! Whisper subprocess backend: serves the Super STT `/v1` contract over a
//! pathname Unix socket (`SUPER_STT_BACKEND_SOCKET`), loading the model from
//! `SUPER_STT_BACKEND_DIR/models/<name>`. Self-contained — no super-stt deps.

#![allow(clippy::doc_markdown)]

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

use super_stt_backend_whisper::inference::WhisperEngine;

#[derive(Clone, Copy)]
enum LoadState {
    Starting,
    Loading,
    Ready,
    Error,
}

impl LoadState {
    fn as_str(self) -> &'static str {
        match self {
            LoadState::Starting => "starting",
            LoadState::Loading => "loading",
            LoadState::Ready => "ready",
            LoadState::Error => "error",
        }
    }
}

struct Status {
    state: LoadState,
    model: Option<String>,
    device: Option<String>,
    reason: Option<String>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: LoadState::Starting,
            model: None,
            device: None,
            reason: None,
        }
    }
}

struct AppState {
    backend_dir: PathBuf,
    status: Mutex<Status>,
    engine: Mutex<Option<WhisperEngine>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let socket = std::env::var("SUPER_STT_BACKEND_SOCKET")
        .context("SUPER_STT_BACKEND_SOCKET must be set")?;
    let backend_dir =
        std::env::var("SUPER_STT_BACKEND_DIR").context("SUPER_STT_BACKEND_DIR must be set")?;

    let state = Arc::new(AppState {
        backend_dir: PathBuf::from(backend_dir),
        status: Mutex::new(Status::default()),
        engine: Mutex::new(None),
    });

    let app = Router::new()
        .route("/v1/ping", get(ping))
        .route("/v1/status", get(get_status))
        .route("/v1/load", post(load))
        .route("/v1/transcribe", post(transcribe))
        .route("/v1/cancel", post(cancel))
        .layer(DefaultBodyLimit::disable())
        .with_state(state);

    if let Some(parent) = std::path::Path::new(&socket).parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).with_context(|| format!("bind {socket}"))?;
    log::info!("whisper backend serving /v1 on {socket}");

    loop {
        let (stream, _) = listener.accept().await?;
        let app = app.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let svc = TowerToHyperService::new(app);
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, svc)
                .await
            {
                log::debug!("connection ended: {e}");
            }
        });
    }
}

async fn ping() -> Json<Value> {
    Json(json!({ "status": "success", "message": "pong" }))
}

async fn get_status(State(s): State<Arc<AppState>>) -> Json<Value> {
    let st = s.status.lock().unwrap();
    let mut out = json!({ "status": "success", "state": st.state.as_str() });
    if let Some(m) = &st.model {
        out["model"] = json!({ "name": m });
    }
    if let Some(d) = &st.device {
        out["device"] = json!(d);
    }
    if let Some(r) = &st.reason {
        out["reason"] = json!(r);
    }
    Json(out)
}

#[derive(Deserialize)]
struct LoadReq {
    name: String,
    #[serde(default)]
    device: Option<String>,
}

async fn load(State(s): State<Arc<AppState>>, Json(req): Json<LoadReq>) -> impl IntoResponse {
    {
        let mut st = s.status.lock().unwrap();
        st.state = LoadState::Loading;
        st.model = Some(req.name.clone());
        st.device = None;
        st.reason = None;
    }
    let dir = s.backend_dir.join("models").join(&req.name);
    let force_cpu = req.device.as_deref() == Some("cpu");
    let model_name = req.name.clone();
    let s2 = Arc::clone(&s);
    tokio::spawn(async move {
        let res = tokio::task::spawn_blocking(move || {
            WhisperEngine::load(&dir, &model_name, force_cpu)
        })
        .await;
        match res {
            Ok(Ok(engine)) => {
                let label = engine.device_label().to_string();
                *s2.engine.lock().unwrap() = Some(engine);
                let mut st = s2.status.lock().unwrap();
                st.device = Some(label);
                st.state = LoadState::Ready;
                log::info!("model loaded; ready");
            }
            Ok(Err(e)) => {
                let mut st = s2.status.lock().unwrap();
                st.state = LoadState::Error;
                st.reason = Some(format!("{e:#}"));
                log::error!("model load failed: {e:#}");
            }
            Err(e) => {
                let mut st = s2.status.lock().unwrap();
                st.state = LoadState::Error;
                st.reason = Some(format!("load task panicked: {e}"));
            }
        }
    });
    (
        StatusCode::ACCEPTED,
        Json(json!({ "status": "success", "message": "Loading started" })),
    )
}

#[derive(Deserialize, Default)]
struct TranscribeOptions {
    #[serde(default)]
    stream_realtime: bool,
}

#[derive(Deserialize)]
struct TranscribeReq {
    audio_data: Vec<f32>,
    #[serde(default)]
    sample_rate: Option<u32>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    options: TranscribeOptions,
}

async fn transcribe(
    State(s): State<Arc<AppState>>,
    _headers: HeaderMap,
    Json(req): Json<TranscribeReq>,
) -> Response {
    if !matches!(s.status.lock().unwrap().state, LoadState::Ready) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "status": "error", "message": "not_ready" })),
        )
            .into_response();
    }
    let sample_rate = req.sample_rate.unwrap_or(16000);
    let audio = req.audio_data;
    let language = req.language;

    if req.options.stream_realtime {
        transcribe_streaming(s, audio, sample_rate, language)
    } else {
        transcribe_oneshot(s, audio, sample_rate, language).await
    }
}

async fn transcribe_oneshot(
    s: Arc<AppState>,
    audio: Vec<f32>,
    sample_rate: u32,
    language: Option<String>,
) -> Response {
    let s2 = Arc::clone(&s);
    let result = tokio::task::spawn_blocking(move || {
        let mut guard = s2.engine.lock().unwrap();
        let engine = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("engine not loaded"))?;
        engine.transcribe(&audio, sample_rate, language.as_deref())
    })
    .await;
    match result {
        Ok(Ok(text)) => (
            StatusCode::OK,
            Json(json!({ "status": "success", "transcription": text })),
        )
            .into_response(),
        Ok(Err(e)) => {
            let msg = format!("{e:#}");
            let code = if msg.contains("unsupported_language") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            let body = if msg.contains("unsupported_language") {
                json!({ "status": "error", "message": "unsupported_language" })
            } else {
                json!({ "status": "error", "message": "inference_failed", "detail": msg })
            };
            (code, Json(body)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(
                json!({ "status": "error", "message": "inference_panicked", "detail": format!("{e}") }),
            ),
        )
            .into_response(),
    }
}

// Owned `s` mirrors `transcribe_oneshot`'s signature for symmetry; clippy
// nags because the body only borrows it.
#[allow(clippy::needless_pass_by_value)]
fn transcribe_streaming(
    s: Arc<AppState>,
    audio: Vec<f32>,
    sample_rate: u32,
    language: Option<String>,
) -> Response {
    let (tx, mut rx) = mpsc::unbounded_channel::<SseFrame>();
    let s2 = Arc::clone(&s);
    let preview_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let mut guard = s2.engine.lock().unwrap();
        let Some(engine) = guard.as_mut() else {
            let _ = tx.send(SseFrame::Error("engine not loaded".to_string()));
            return;
        };
        let result = engine.transcribe_streaming(&audio, sample_rate, language.as_deref(), |t| {
            let _ = preview_tx.send(SseFrame::Preview(t.to_string()));
        });
        match result {
            Ok(text) => {
                let _ = tx.send(SseFrame::Done(text));
            }
            Err(e) => {
                let _ = tx.send(SseFrame::Error(format!("{e:#}")));
            }
        }
    });

    let stream = async_stream::stream! {
        while let Some(frame) = rx.recv().await {
            let terminal = frame.is_terminal();
            yield Ok::<_, Infallible>(frame.encode());
            if terminal {
                break;
            }
        }
    };

    let body = Body::from_stream(stream);
    let mut resp = Response::new(body);
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    resp
}

enum SseFrame {
    Preview(String),
    Done(String),
    Error(String),
}

impl SseFrame {
    fn encode(&self) -> bytes::Bytes {
        let s = match self {
            Self::Preview(t) => format!(
                "event: preview\ndata: {}\n\n",
                serde_json::to_string(&json!({ "text": t })).unwrap()
            ),
            Self::Done(t) => format!(
                "event: done\ndata: {}\n\n",
                serde_json::to_string(&json!({ "transcription": t })).unwrap()
            ),
            Self::Error(m) => format!(
                "event: error\ndata: {}\n\n",
                serde_json::to_string(&json!({ "message": m })).unwrap()
            ),
        };
        bytes::Bytes::from(s)
    }

    fn is_terminal(&self) -> bool {
        matches!(self, Self::Done(_) | Self::Error(_))
    }
}

async fn cancel() -> Json<Value> {
    Json(json!({ "status": "success", "message": "Cancelled" }))
}
