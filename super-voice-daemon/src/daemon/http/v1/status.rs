// SPDX-License-Identifier: GPL-3.0-only
//! `/status` — what the daemon is currently running.
//!
//! Contract: `docs/protocol/endpoints/v1/status.md`.
//!
//! A summary for a client with the `status` scope alone, one per feature: the
//! loaded model, the device under it, and whether the feature is at work. The
//! per-stage detail behind it is `settings`-scoped, under each feature's
//! pipeline.

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use axum::extract::State;
use axum::response::Response;
use super_stt_shared::models::status::{FeatureStatus, VoiceStatus};

#[utoipa::path(
    get,
    path = "/status",
    tag = "health",
    summary = "Each feature's model and device",
    description = "\
A snapshot of what the daemon is running, one part per feature: which model is \
loaded, on which accelerator, and whether the feature is at work — a recording \
cycle for `stt`, an utterance for `tts`.

Subscriber introspection and other operator detail are not exposed here — see \
`GET /stt/pipeline/1` and `GET /tts/pipeline/1`, which need the `settings` scope.",
    security(("session_token" = ["status"])),
    responses(
        (status = 200, description = "The daemon's current state.", body = VoiceStatus),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `status` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn status(State(s): State<AppState>) -> Response {
    let stt = dispatch(&s.daemon, build_request("status", None)).await;
    let tts = s.daemon.tts.handle_status().await;
    // Each `handle_status` fills all four on every success; the fallbacks keep
    // the endpoint answering its own shape rather than a partial one if a
    // future command path ever forgets.
    let tts = FeatureStatus {
        device: tts.device.unwrap_or_else(|| "unknown".to_string()),
        model_loaded: tts.model_loaded.unwrap_or(false),
        current_model: tts.current_model,
        busy: tts.busy.unwrap_or(false),
    };
    narrowed(stt, |r| VoiceStatus {
        status: "success".to_string(),
        stt: FeatureStatus {
            device: r.device.unwrap_or_else(|| "unknown".to_string()),
            model_loaded: r.model_loaded.unwrap_or(false),
            current_model: r.current_model,
            busy: r.busy.unwrap_or(false),
        },
        tts,
    })
}
