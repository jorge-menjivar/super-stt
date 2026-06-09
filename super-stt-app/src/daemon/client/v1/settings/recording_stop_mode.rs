// SPDX-License-Identifier: GPL-3.0-only
//! `/recording_stop_mode` — how recording stops (silence, manual, or both).

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;

/// Get recording stop mode (HTTP `GET /recording_stop_mode`).
pub async fn get_recording_stop_mode() -> Result<String, String> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/recording_stop_mode").await?,
            "get_stop_mode",
        )?;
        Ok(resp
            .recording_stop_mode
            .unwrap_or_else(|| "silence-and-manual".to_string()))
    })
    .await
}

/// Set recording stop mode (HTTP `POST /recording_stop_mode`).
pub async fn set_recording_stop_mode(mode: String) -> Result<(), String> {
    with_settings_token(move |socket, token| {
        let mode = mode.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                "/recording_stop_mode",
                &serde_json::json!({ "mode": mode }),
            )
            .await?;
            require_unit(resp, "set_stop_mode")
        }
    })
    .await
}
