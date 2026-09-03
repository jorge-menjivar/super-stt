// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::auth::middleware::AuthContext;
use crate::daemon::http::internal::helpers::responses::{invalid_session, reason};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// What the token currently held is good for.
#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct AuthStatusOk {
    /// Always `success`.
    #[schema(example = "success")]
    pub(crate) status: &'static str,
    /// The scopes the token was minted under.
    pub(crate) scopes: Vec<String>,
    /// RFC 3339 expiry, so a headless client can renew before it lapses.
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
#[utoipa::path(
    get,
    path = "/auth/status",
    tag = "auth",
    summary = "Check the held token without prompting",
    description = "\
Reports what the presented token is good for, and never opens a consent popup — \
which is what makes it the right probe for a headless or CLI client. Reaching this \
handler at all means the token validated.

Use it to fail fast on a token about to expire, rather than discovering it \
mid-operation.",
    security(("session_token" = [])),
    responses(
        (status = 200, description = "The token is valid.", body = AuthStatusOk),
        (status = 401, description = "Token unknown, expired, or its binary changed — re-run the consent handshake.", body = ReasonEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
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
