// SPDX-License-Identifier: GPL-3.0-only
use super::super::super::internal::error::HttpResult;
use super::super::super::internal::transport;
use serde::Deserialize;
use std::path::PathBuf;

/// Response shape for [`auth_status`]. `status` is `"success"` on the
/// valid-token path; an invalid token surfaces as an `Err` from the
/// caller, not a `status: "error"` body.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthStatusInfo {
    pub status: String,
    pub scopes: Vec<String>,
    pub expires_at: String,
}

/// `GET /auth/status` — probe whether the held token is still valid.
///
/// Returns the granted scopes + RFC 3339 expiry on success. On invalid token,
/// the underlying request returns `Err("invalid_session (<reason>)")`
/// — same shape as any other 401 from the daemon — so callers can
/// switch on `contains("invalid_session")` to trigger a re-auth.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable, the
/// token is invalid, or the response can't be parsed.
pub async fn auth_status(socket_path: PathBuf, token: &str) -> HttpResult<AuthStatusInfo> {
    let req = transport::build_get("/auth/status", Some(token))?;
    transport::send_request::<AuthStatusInfo>(&socket_path, req).await
}
