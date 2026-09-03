// SPDX-License-Identifier: GPL-3.0-only
//! Liveness, and what the daemon is currently running.

use crate::daemon::http::internal::helpers::dispatch::{ack, build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{Ack, ErrorEnvelope, ReasonEnvelope};
use axum::extract::State;
use axum::response::Response;
use serde::Serialize;
use utoipa::ToSchema;

/// What `GET /status` reports.
#[derive(Serialize, ToSchema)]
pub(crate) struct DaemonStatus {
    /// Always `success`.
    #[schema(example = "success")]
    status: &'static str,
    /// The accelerator the loaded model actually runs on: `cpu`, `cuda`,
    /// `rocm`, `metal`, `vulkan`, or `remote` for a model served over the
    /// network. `unknown` when nothing is loaded.
    #[schema(example = "cuda")]
    device: String,
    /// `false` while the initial model is still loading, or after a failed
    /// switch.
    model_loaded: bool,
    /// The loaded model's name. Absent when `model_loaded` is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "whisper-tiny")]
    current_model: Option<String>,
    /// `true` while a daemon-mic cycle is active — capture *and* the
    /// transcription and typing that follow it. A toggle hotkey reads this and
    /// calls `POST /transcribe/stop` when true, `POST /transcribe` when false.
    busy: bool,
}

#[utoipa::path(
    get,
    path = "/ping",
    tag = "health",
    summary = "Liveness probe",
    description = "\
Confirms the listener is reachable and the presented token is valid. Introspects \
no state.

To check whether the *token* is still good without risking a consent popup, prefer \
`GET /auth/status` — that is the dedicated probe.",
    security(("session_token" = [])),
    responses(
        (status = 200, description = "The daemon is up. `message` is always `pong`.", body = Ack,
         example = json!({ "status": "success", "message": "pong" })),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "Connection refused — the daemon is over its per-client connection cap.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn ping(State(s): State<AppState>) -> Response {
    ack(&s.daemon, "ping", None).await
}

#[utoipa::path(
    get,
    path = "/status",
    tag = "health",
    summary = "Current model and device",
    description = "\
A snapshot of what the daemon is running: which model is loaded, on which \
accelerator, and whether a recording cycle is in flight.

Subscriber introspection and other operator detail are not exposed here — see \
`GET /pipeline/1` and `GET /pipeline/{stage}/model/{model}/device`, which need the \
`settings` scope.",
    security(("session_token" = ["status"])),
    responses(
        (status = 200, description = "The daemon's current state.", body = DaemonStatus),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `status` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn status(State(s): State<AppState>) -> Response {
    let resp = dispatch(&s.daemon, build_request("status", None)).await;
    narrowed(resp, |r| DaemonStatus {
        status: "success",
        // `handle_status` fills all three on every success; the fallbacks keep
        // the endpoint answering its own shape rather than a partial one if a
        // future command path ever forgets.
        device: r.device.unwrap_or_else(|| "unknown".to_string()),
        model_loaded: r.model_loaded.unwrap_or(false),
        current_model: r.current_model,
        busy: r.busy.unwrap_or(false),
    })
}
