// SPDX-License-Identifier: GPL-3.0-only
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

// The error bodies the guards and `/events` answer with are the engine's,
// re-exported for the envelope contract tests, which hold every error shape
// the daemon sends against the ones it documents.
#[cfg(test)]
pub(crate) use super_engine_daemon::http::responses::{
    invalid_session, rate_limited, reason, scope_denied,
};

/// Pre-built `409 recording_in_progress` JSON response for the
/// `/v1/transcribe` handler. Clients should check `GET /v1/status`
/// for `busy` and call `/v1/transcribe/stop` instead of
/// retrying `/v1/transcribe`.
pub(crate) fn recording_in_progress_response() -> Response {
    let body = serde_json::json!({
        "status":  "error",
        "message": "recording_in_progress",
    });
    (
        StatusCode::CONFLICT,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// Pre-built `409 model_not_loaded` JSON response for the `/v1/transcribe`
/// handler's daemon-mic paths. Mirrors [`recording_in_progress_response`]
/// (same shape, same literal identifier as `message`): returned before the
/// `202`/`200 text/event-stream` envelope below would otherwise commit, so
/// the documented `409` is actually reachable
/// (`docs/protocol/endpoints/v1/transcribe.md`). Load a model via
/// `POST /active_model` and retry.
///
/// Carries `error_code: "model_not_loaded"` so this shape agrees with the
/// pre-captured `audio_data` path's `409` for the same condition (built via
/// `DaemonResponse::error_with_code(ErrorCode::ModelNotLoaded, ..)`) — both
/// are the same documented error and must expose the same stable,
/// machine-readable identifier (`docs/protocol/transport.md`).
pub(crate) fn model_not_loaded_response() -> Response {
    let body = serde_json::json!({
        "status":     "error",
        "error_code": "model_not_loaded",
        "message":    "model_not_loaded",
    });
    (
        StatusCode::CONFLICT,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}
