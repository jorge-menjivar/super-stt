// SPDX-License-Identifier: GPL-3.0-only
//! `/ping` — daemon connectivity checks.
//!
//! Named for the path, like every other endpoint module. The two calls differ
//! only in what they keep: one wants the daemon's reply text, the other only
//! that a reply arrived at all.

use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client;
use super_tts_shared::daemon::http_client::HttpResult;

/// Test daemon connection (HTTP `GET /ping`).
pub async fn test_daemon_connection() -> HttpResult<()> {
    with_settings_token(|socket, token| async move {
        http_client::ping(socket, &token).await.map(|_| ())
    })
    .await
}

/// Ping daemon to check connectivity (HTTP `GET /ping`).
pub async fn ping_daemon() -> HttpResult<String> {
    with_settings_token(|socket, token| async move { http_client::ping(socket, &token).await })
        .await
}
