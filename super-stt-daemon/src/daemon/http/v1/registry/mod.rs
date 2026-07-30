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

/// Registry error envelope: `{ "status": "error", "error_code": <code>, "error": <code> }`
/// at `status`.
///
/// The registry endpoints historically used a bare `{ "error": <code> }` shape.
/// `error_code` (and `status`) are now emitted alongside the retained `error`
/// key so the whole surface honors `transport.md`'s "`error_code` present on
/// every error" contract without breaking clients that read `error` (audit 2
/// Tier 2 #6). This helper is the single place that shape is built.
pub(crate) fn registry_error(status: StatusCode, code: &str) -> Response {
    (
        status,
        [("content-type", "application/json")],
        serde_json::json!({ "status": "error", "error_code": code, "error": code }).to_string(),
    )
        .into_response()
}

/// [`registry_error`] with a human-readable `message`.
pub(crate) fn registry_error_msg(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        [("content-type", "application/json")],
        serde_json::json!({
            "status": "error",
            "error_code": code,
            "error": code,
            "message": message,
        })
        .to_string(),
    )
        .into_response()
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

    /// The registry error envelope now carries the machine-readable `error_code`
    /// (and `status`) per transport.md, while retaining the legacy `error` key so
    /// existing clients keep working (audit 2 Tier 2 #6).
    #[tokio::test]
    async fn registry_error_carries_error_code_and_legacy_key() {
        let (status, body) = body_of(registry_error(StatusCode::NOT_FOUND, "not_found")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let v: serde_json::Value = serde_json::from_str(&body).expect("json");
        assert_eq!(v["status"], "error");
        assert_eq!(v["error_code"], "not_found");
        assert_eq!(v["error"], "not_found"); // retained for back-compat

        let (status, body) = body_of(registry_error_msg(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "provide exactly one of source, repo_url, local_path",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let v: serde_json::Value = serde_json::from_str(&body).expect("json");
        assert_eq!(v["status"], "error");
        assert_eq!(v["error_code"], "bad_request");
        assert_eq!(v["error"], "bad_request");
        assert_eq!(
            v["message"],
            "provide exactly one of source, repo_url, local_path"
        );
    }
}
