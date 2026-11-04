// SPDX-License-Identifier: GPL-3.0-only
//! `/registry/backend/refresh` — re-fetch the published index.
//!
//! The daemon caches the index, so the Download tab can render without a
//! network round trip. This is what a Refresh button calls when the user has
//! reason to think the cache is behind — a backend published minutes ago.

use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;
use super_tts_shared::registry::RefreshResponse;

/// `POST /registry/backend/refresh` — ask the daemon to re-fetch the remote
/// index and return summary counts.
pub async fn refresh() -> HttpResult<RefreshResponse> {
    with_settings_token(|socket, token| async move {
        transport::post_json::<RefreshResponse>(
            socket,
            &token,
            "/registry/backend/refresh",
            &serde_json::json!({}),
        )
        .await
    })
    .await
}
