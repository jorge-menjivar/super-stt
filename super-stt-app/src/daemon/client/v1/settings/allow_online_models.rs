// SPDX-License-Identifier: GPL-3.0-only
//! `/allow_online_models` — gate for network-fetched model inference.

use crate::daemon::client::internal::response::require_unit;
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;

/// Set allow-online-models flag (HTTP `POST /allow_online_models`).
pub async fn set_allow_online_models(enabled: bool) -> Result<(), String> {
    with_settings_token(move |socket, token| async move {
        let resp = transport::settings_post(
            socket,
            &token,
            "/allow_online_models",
            &serde_json::json!({ "enabled": enabled }),
        )
        .await?;
        require_unit(resp, "set_allow_online")
    })
    .await
}
