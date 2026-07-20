// SPDX-License-Identifier: GPL-3.0-only
use super::super::super::internal::error::{HttpError, HttpResult};
use super::super::super::internal::transport;
use serde::Deserialize;
use std::path::PathBuf;

/// Successful `POST /auth/request` payload.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthOk {
    pub session_token: String,
    pub scopes: Vec<String>,
    pub expires_at: String,
}

/// `POST /auth/request` — always runs the consent popup and mints a
/// fresh session token. Used by clients that have no cached token (or
/// whose cached token was invalidated by `401 invalid_session`).
/// Clients with a valid cached token never call this; they go
/// straight to `/ping`/`/events`/etc. with the bearer header.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable, the
/// user denies the request, or the popup is dismissed.
pub async fn auth_request(
    socket_path: PathBuf,
    app_name: &str,
    scopes: &[&str],
) -> HttpResult<AuthOk> {
    let body = serde_json::json!({
        "app_name": app_name,
        "scopes":   scopes,
        "version":  env!("CARGO_PKG_VERSION"),
    });
    let req = transport::build_post_json("/auth/request", &body, None)?;

    // /auth/request returns its own JSON shape (not a `DaemonResponse`),
    // so we issue the request directly here and parse on top of the raw
    // body. No timeout: the daemon holds this request open while the user
    // responds to the consent popup (and may type a keyring password), so
    // a machine timer must not race human input — bounding it would cut
    // the user off mid-decision.
    let response = transport::open(&socket_path, req, None).await?;

    let status = response.status();
    let body = transport::collect_body(response).await?;

    // Deliberately not `transport::error_for_status`: a 4xx here means the user
    // declined consent (or the daemon refused to ask), which callers handle as
    // `AuthDenied` with a reason — not as an operational daemon error.
    if !status.is_success() {
        return Err(HttpError::AuthDenied {
            reason: transport::parse_reason(&body, "auth_denied"),
        });
    }

    serde_json::from_slice::<AuthOk>(&body)
        .map_err(|e| HttpError::Other(format!("Failed to parse auth_ok: {e}")))
}
