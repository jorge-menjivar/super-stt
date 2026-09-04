// SPDX-License-Identifier: GPL-3.0-only
//! `/audio_themes` — the themes the daemon ships.
//!
//! A sibling path of [`super::audio_theme`], not a sub-path of it: this lists
//! what is on offer, that one holds the selection.

use crate::daemon::client::internal::response::require_success;
use crate::daemon::client::internal::session::with_settings_token;
use crate::state::AudioTheme;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

/// Load available audio themes from daemon, falling back to the built-in set.
pub async fn load_audio_themes() -> Vec<AudioTheme> {
    list_available_audio_themes()
        .await
        .unwrap_or_else(|_| AudioTheme::all_themes())
}

/// List available audio themes (HTTP `GET /audio_themes`).
pub async fn list_available_audio_themes() -> HttpResult<Vec<AudioTheme>> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/audio_themes").await?,
            "list_themes",
        )?;
        let themes = resp
            .available_audio_themes
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.to_string())
            .filter_map(|s| s.parse::<AudioTheme>().ok())
            .collect();
        Ok(themes)
    })
    .await
}
