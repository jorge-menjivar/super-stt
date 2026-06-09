// SPDX-License-Identifier: GPL-3.0-only
//! `/ping` — daemon connectivity checks.

use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client;

/// Test daemon connection (HTTP `/ping`).
pub async fn test_daemon_connection() -> Result<(), String> {
    with_settings_token(|socket, token| async move {
        http_client::ping(socket, &token)
            .await
            .map(|_| ())
            .map_err(String::from)
    })
    .await
}

/// Ping daemon to check connectivity (HTTP `/ping`).
pub async fn ping_daemon() -> Result<String, String> {
    with_settings_token(|socket, token| async move {
        http_client::ping(socket, &token)
            .await
            .map_err(String::from)
    })
    .await
}
