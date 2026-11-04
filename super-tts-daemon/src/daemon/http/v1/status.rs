// SPDX-License-Identifier: GPL-3.0-only
//! `/status` — what the daemon is currently running.
//!
//! Contract: `docs/protocol/endpoints/v1/status.md`.
//!
//! A summary for a client holding the `status` scope alone: the loaded model,
//! the device under it, and whether an utterance is in flight. The per-stage
//! detail behind it is `settings`-scoped, in [`super::pipeline`].

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
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
    #[schema(example = "kokoro-82m")]
    current_model: Option<String>,
    /// `true` while an utterance is being synthesized or played out. A
    /// speak/silence toggle reads this and calls `POST /speak/stop` when true,
    /// `POST /speak` when false. `POST /speak` itself is not gated on it — a
    /// new utterance deliberately interrupts the current one.
    busy: bool,
}

#[utoipa::path(
    get,
    path = "/status",
    tag = "health",
    summary = "Current model, device and speech state",
    description = "\
A snapshot of what the daemon is running: which model is loaded, on which \
accelerator, and whether it is speaking right now.

`busy` is about playback, not about the request queue — it is `true` from the \
moment an utterance starts being synthesized until the last sample has been \
played out. It is what a toggle shortcut consults to decide between `POST /speak` \
and `POST /speak/stop`.

Operator detail is deliberately not here. Which backend fills the synthesis stage, \
and what device that stage's model was asked for, are `GET /pipeline/1` and \
`GET /pipeline/{stage}/model/{model}/device`, both of which need the `settings` \
scope.",
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
        // `handle_status` fills all four on every success; the fallbacks keep
        // the endpoint answering its own shape rather than a partial one if a
        // future command path ever forgets.
        device: r.device.unwrap_or_else(|| "unknown".to_string()),
        model_loaded: r.model_loaded.unwrap_or(false),
        current_model: r.current_model,
        busy: r.busy.unwrap_or(false),
    })
}
