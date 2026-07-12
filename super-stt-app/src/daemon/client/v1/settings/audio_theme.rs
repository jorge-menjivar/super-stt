// SPDX-License-Identifier: GPL-3.0-only
//! `/audio_theme(s)` — audio-cue theme selection and audition.

use crate::daemon::client::internal::response::{require_message, require_success};
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

/// Read the configured audio-cue theme (HTTP `GET /audio_theme`).
pub async fn get_current_audio_theme() -> HttpResult<AudioTheme> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/audio_theme").await?,
            "get_audio_theme",
        )?;
        Ok(resp
            .audio_theme
            .unwrap_or_default()
            .parse()
            .unwrap_or_default())
    })
    .await
}

/// Set audio theme without playing a test sound (HTTP `POST /audio_theme`).
pub async fn set_audio_theme(theme: AudioTheme) -> HttpResult<String> {
    let theme_str = theme.to_string().to_lowercase();
    with_settings_token(move |socket, token| {
        let theme_str = theme_str.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                "/audio_theme",
                &serde_json::json!({ "theme": theme_str }),
            )
            .await?;
            require_message(resp, "set_theme")
        }
    })
    .await
}

/// Set then audition a theme (`POST /audio_theme` + `POST /audio_theme/test`).
pub async fn set_and_test_audio_theme(theme: AudioTheme) -> HttpResult<String> {
    set_audio_theme(theme).await?;
    with_settings_token(|socket, token| async move {
        let resp =
            transport::settings_post(socket, &token, "/audio_theme/test", &serde_json::json!({}))
                .await?;
        require_message(resp, "test_theme")
    })
    .await
}
