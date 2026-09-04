// SPDX-License-Identifier: GPL-3.0-only
//! `/audio_theme` — the selected audio cue theme, and a preview of it.
//!
//! The themes on offer are listed at [`/audio_themes`](super::audio_themes).

use crate::daemon::client::internal::response::{require_message, require_success};
use crate::daemon::client::internal::session::with_settings_token;
use crate::state::AudioTheme;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

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
