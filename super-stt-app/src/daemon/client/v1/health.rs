// SPDX-License-Identifier: GPL-3.0-only
//! `/ping` — daemon connectivity checks.

use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client;
use super_stt_shared::daemon::http_client::HttpResult;

/// Test daemon connection (HTTP `/ping`).
pub async fn test_daemon_connection() -> HttpResult<()> {
    with_settings_token(|socket, token| async move {
        http_client::ping(socket, &token).await.map(|_| ())
    })
    .await
}

/// Ping daemon to check connectivity (HTTP `/ping`).
pub async fn ping_daemon() -> HttpResult<String> {
    with_settings_token(|socket, token| async move { http_client::ping(socket, &token).await })
        .await
}
