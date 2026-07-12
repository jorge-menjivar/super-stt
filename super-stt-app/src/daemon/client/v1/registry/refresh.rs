// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;
use super_stt_shared::registry::RefreshResponse;

/// `POST /registry/backends/refresh` — ask the daemon to re-fetch the remote
/// index and return summary counts.
pub async fn refresh() -> HttpResult<RefreshResponse> {
    with_settings_token(|socket, token| async move {
        transport::post_json::<RefreshResponse>(
            socket,
            &token,
            "/registry/backends/refresh",
            &serde_json::json!({}),
        )
        .await
    })
    .await
}
