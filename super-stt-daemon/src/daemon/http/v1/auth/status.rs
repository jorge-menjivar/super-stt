// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::auth::middleware::AuthContext;
use crate::daemon::http::internal::helpers::responses::{invalid_session, reason};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct AuthStatusOk {
    pub(crate) status: &'static str,
    pub(crate) scopes: Vec<String>,
    pub(crate) expires_at: String,
}

/// `GET /v1/auth/status` — no-side-effect probe that the bearer token
/// is still valid. The `require_any_authenticated` middleware has
/// already validated the token and inserted [`AuthContext`] into the
/// request extensions by the time this handler runs, so reaching the
/// handler at all means the token was good. The handler reports back
/// the scope set it was minted under and the expiry timestamp so a
/// headless / CLI client can fail-fast on a soon-to-expire token
/// without invoking the consent UI.
///
/// Errors (`401 invalid_session` with `data.reason` of `unknown`,
/// `expired`, or `exe_changed`) are produced upstream by
/// `require_any_authenticated` before this handler runs.
pub(crate) async fn auth_status(ctx: Option<axum::Extension<AuthContext>>) -> Response {
    let Some(axum::Extension(ctx)) = ctx else {
        return invalid_session(reason::UNKNOWN);
    };
    let payload = AuthStatusOk {
        status: "success",
        scopes: ctx.meta.scopes.clone(),
        expires_at: ctx.meta.expires_at.to_rfc3339(),
    };
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string(&payload).unwrap_or_default(),
    )
        .into_response()
}
