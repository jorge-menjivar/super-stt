// SPDX-License-Identifier: GPL-3.0-only
pub(crate) mod model_language;
pub(crate) mod options;
pub(crate) mod secrets;

use crate::daemon::http::state::AppState;
use crate::stt_models::backends::DiscoveredBackend;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Percent-decode a `{source}` path segment (e.g. `github.com%2Facme%2Fx`).
pub(crate) fn decode_source(raw: &str) -> String {
    urlencoding::decode(raw).map_or_else(|_| raw.to_string(), std::borrow::Cow::into_owned)
}

/// Clone the catalog entry for `source`, if installed.
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
/// backend endpoints carry no separate human-readable text (audit 2 Tier 2 #6).
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

/// House-style JSON success response with status 200.
pub(crate) fn ok(v: &serde_json::Value) -> Response {
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        v.to_string(),
    )
        .into_response()
}
