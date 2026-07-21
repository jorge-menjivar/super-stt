// SPDX-License-Identifier: GPL-3.0-only
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Canonical error response builder for the `{ "status": "error",
/// "message": <message>, "data": { "reason": <reason> } }` shape.
///
/// All error responses that carry a `data.reason` field route through
/// this function so the JSON shape is defined in exactly one place.
pub(crate) fn error_response(status: StatusCode, message: &str, reason: &str) -> Response {
    let body = serde_json::json!({
        "status":  "error",
        "message": message,
        "data":    { "reason": reason }
    });
    (
        status,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// Wire-level `reason` string constants used in `data.reason` fields.
/// Values are the exact `snake_case` strings the protocol specifies;
/// callers MUST use these rather than inline literals.
pub(crate) mod reason {
    // invalid_session reasons
    pub(crate) const UNKNOWN: &str = "unknown";

    // auth_denied reasons
    pub(crate) const INVALID_BODY: &str = "invalid_body";
    pub(crate) const INVALID_SCOPE: &str = "invalid_scope";
    pub(crate) const UID_MISMATCH: &str = "uid_mismatch";
    pub(crate) const USER_DENIED_CACHED: &str = "user_denied_cached";
    pub(crate) const USER_DENIED: &str = "user_denied";
    pub(crate) const USER_DISMISSED: &str = "user_dismissed";
    pub(crate) const POPUP_FAILED: &str = "popup_failed";
    /// The daemon could not resolve the peer's executable (`SO_PEERCRED`/pid
    /// missing, or `/proc/<pid>/exe` unreadable), so it can't verify *which*
    /// binary is asking — consent requires a verifiable binary, so it fails
    /// closed (audit 2 Tier 3 #9).
    pub(crate) const PEER_UNVERIFIABLE: &str = "peer_unverifiable";
}

pub(crate) fn invalid_session(reason: &'static str) -> Response {
    error_response(StatusCode::UNAUTHORIZED, "invalid_session", reason)
}

pub(crate) fn scope_denied() -> Response {
    let body = serde_json::json!({
        "status":  "error",
        "message": "scope_denied",
    });
    (
        StatusCode::FORBIDDEN,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

pub(crate) fn rate_limited() -> Response {
    let body = serde_json::json!({
        "status":  "error",
        "message": "rate_limited",
    });
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

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

pub(crate) fn auth_err(status: StatusCode, message: &str, reason: &str) -> Response {
    error_response(status, message, reason)
}

/// The auth scope catalog now lives in `super-stt-shared` so the daemon and the
/// consent dialog share one list (Tier 2 #8). Re-exported for the existing
/// `/auth/request` validation call site; the catalog's own tests live in
/// `super_stt_shared::daemon::scopes`.
pub(crate) use super_stt_shared::daemon::scopes::is_known_scope;
