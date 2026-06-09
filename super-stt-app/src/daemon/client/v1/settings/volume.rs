// SPDX-License-Identifier: GPL-3.0-only
//! `/volume` — audio-cue master volume (0–100).

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;

/// Read current cue volume (HTTP `GET /volume`).
pub async fn get_volume() -> Result<u8, String> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/volume").await?,
            "get_volume",
        )?;
        // The daemon reports volume in the `message` field as text
        // ("Volume is 75"); parse the trailing integer.
        let vol = resp
            .message
            .unwrap_or_default()
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(100);
        Ok(vol)
    })
    .await
}

/// Set master volume (HTTP `POST /volume`).
pub async fn set_volume(volume: u8) -> Result<(), String> {
    with_settings_token(move |socket, token| async move {
        let resp = transport::settings_post(
            socket,
            &token,
            "/volume",
            &serde_json::json!({ "volume": volume }),
        )
        .await?;
        require_unit(resp, "set_volume")
    })
    .await
}
