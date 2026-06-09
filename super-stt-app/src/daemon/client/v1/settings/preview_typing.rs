// SPDX-License-Identifier: GPL-3.0-only
//! `/preview_typing` — enable/disable live preview typing during recording.

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;

/// Get preview-typing flag (HTTP `GET /preview_typing`).
pub async fn get_preview_typing() -> Result<bool, String> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/preview_typing").await?,
            "get_preview_typing",
        )?;
        Ok(resp.preview_typing_enabled.unwrap_or(false))
    })
    .await
}

/// Set preview-typing flag (HTTP `POST /preview_typing`).
pub async fn set_preview_typing(enabled: bool) -> Result<(), String> {
    with_settings_token(move |socket, token| async move {
        let resp = transport::settings_post(
            socket,
            &token,
            "/preview_typing",
            &serde_json::json!({ "enabled": enabled }),
        )
        .await?;
        require_unit(resp, "set_preview_typing")
    })
    .await
}
