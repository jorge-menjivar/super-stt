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

pub(crate) fn auth_err(status: StatusCode, message: &str, reason: &str) -> Response {
    error_response(status, message, reason)
}

/// The complete set of scope tokens this daemon understands, in wire
/// (`snake_case`) form. A token may be granted any non-empty subset.
/// Source of truth for `/auth/request` validation; mirrors the scope
/// catalog in `docs/protocol/auth.md`.
pub(crate) const KNOWN_SCOPES: &[&str] = &[
    "transcribe",
    "status",
    "settings",
    "recording_events",
    "audio_visualization",
    "global_transcriptions",
    "daemon_status",
];

/// True if `s` is a recognized scope token.
pub(crate) fn is_known_scope(s: &str) -> bool {
    KNOWN_SCOPES.contains(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_scopes_are_recognized() {
        for s in KNOWN_SCOPES {
            assert!(is_known_scope(s), "{s} should be a known scope");
        }
    }

    #[test]
    fn old_personas_and_garbage_are_rejected() {
        for s in ["client", "widget", "", "Settings", "transcribe ", "global"] {
            assert!(!is_known_scope(s), "{s:?} must not be a known scope");
        }
    }
}
