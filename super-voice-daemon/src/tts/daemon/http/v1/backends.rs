// SPDX-License-Identifier: GPL-3.0-only
//! What text to speech's routes need of a backend: finding the one a request
//! names, and the house-style error envelope. The `/v1/backend` routes
//! themselves are the daemon's, served once for both features.

use crate::tts::daemon::http::state::AppState;
use crate::tts::tts_models::backends::DiscoveredBackend;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Clone the synthesis catalog entry for `source`, if installed.
pub(crate) async fn find_backend(s: &AppState, source: &str) -> Option<DiscoveredBackend> {
    s.daemon
        .backends
        .read()
        .await
        .iter()
        .find(|b| b.source == source)
        .cloned()
}

/// House-style JSON error envelope at a given status. `error_code` is the stable
/// machine-readable `snake_case` identifier clients switch on (per `transport.md`,
/// "present on every error"); it is also mirrored into `message` since these
/// endpoints carry no separate human-readable text.
pub(crate) fn json_error(code: StatusCode, error_code: &str) -> Response {
    json_error_msg(code, error_code, error_code)
}

/// [`json_error`] with a distinct human-readable `message` (the machine
/// identifier still rides in `error_code`).
pub(crate) fn json_error_msg(code: StatusCode, error_code: &str, message: &str) -> Response {
    (
        code,
        [("content-type", "application/json")],
        serde_json::json!({ "status": "error", "error_code": error_code, "message": message })
            .to_string(),
    )
        .into_response()
}
