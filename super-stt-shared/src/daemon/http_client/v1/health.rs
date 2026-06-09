// SPDX-License-Identifier: GPL-3.0-only
use super::super::internal::error::HttpResult;
use super::super::internal::transport;
use crate::models::protocol::DaemonResponse;
use std::path::PathBuf;

/// `GET /ping` — liveness check.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable or the
/// response can't be parsed.
pub async fn ping(socket_path: PathBuf, token: &str) -> HttpResult<String> {
    let req = transport::build_get("/ping", Some(token))?;
    let resp = transport::send_request::<DaemonResponse>(&socket_path, req).await?;
    Ok(resp
        .message
        .unwrap_or_else(|| "Daemon is running".to_string()))
}

/// `GET /status` — current model + device.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable or the
/// response can't be parsed.
pub async fn status(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    let req = transport::build_get("/status", Some(token))?;
    transport::send_request::<DaemonResponse>(&socket_path, req).await
}
