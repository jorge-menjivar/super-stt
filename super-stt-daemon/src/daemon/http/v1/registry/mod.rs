// SPDX-License-Identifier: GPL-3.0-only
pub(crate) mod install;
pub(crate) mod list;
pub(crate) mod pipeline;
pub(crate) mod refresh;
pub(crate) mod update;

use crate::daemon::http::state::AppState;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

/// Registry error envelope: `{ "error": <code> }` at `status`.
///
/// The registry endpoints deliberately use a stable, machine-readable `error`
/// code (see the failure-mode tables in
/// `docs/protocol/endpoints/v1/registry/*.md`) rather than the
/// `{ "status": "error", "message": … }` house style used elsewhere. This
/// helper is the single place that shape is built.
pub(crate) fn registry_error(status: StatusCode, code: &str) -> Response {
    (
        status,
        [("content-type", "application/json")],
        serde_json::json!({ "error": code }).to_string(),
    )
        .into_response()
}

/// Registry error envelope with a human-readable `message`:
/// `{ "error": <code>, "message": <message> }`.
pub(crate) fn registry_error_msg(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        [("content-type", "application/json")],
        serde_json::json!({ "error": code, "message": message }).to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{registry_error, registry_error_msg};
    use axum::http::StatusCode;

    async fn body_of(resp: axum::response::Response) -> (StatusCode, String) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("collect body");
        (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
    }

    /// The registry error envelope is the documented `{ "error": <code> }`
    /// machine-readable shape (see docs/protocol/endpoints/v1/registry/*.md) —
    /// distinct from the `{ "status": "error", "message": … }` house style.
    #[tokio::test]
    async fn registry_error_uses_documented_envelope() {
        let (status, body) = body_of(registry_error(StatusCode::NOT_FOUND, "not_found")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, r#"{"error":"not_found"}"#);

        let (status, body) = body_of(registry_error_msg(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "provide exactly one of source, repo_url, local_path",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            r#"{"error":"bad_request","message":"provide exactly one of source, repo_url, local_path"}"#
        );
    }
}

/// Registry browse/refresh/install/update routes (settings-scope).
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/registry/backends", get(list::list_registry_backends))
        .route(
            "/registry/backends/refresh",
            post(refresh::refresh_registry),
        )
        .route(
            "/registry/backends/install",
            post(install::install_registry_backend),
        )
        .route(
            "/registry/backends/update",
            post(update::update_registry_backend),
        )
}
