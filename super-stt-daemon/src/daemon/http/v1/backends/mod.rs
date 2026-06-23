// SPDX-License-Identifier: GPL-3.0-only
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

/// House-style JSON error envelope at a given status.
pub(crate) fn json_error(code: StatusCode, message: &str) -> Response {
    (
        code,
        [("content-type", "application/json")],
        serde_json::json!({ "status": "error", "message": message }).to_string(),
    )
        .into_response()
}
