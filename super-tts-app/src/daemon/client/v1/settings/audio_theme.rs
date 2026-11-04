// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/audio_theme` — audio-cue theme selection, listing, and audition.
//!
//! The cues are the short tones that mark the start and end of an utterance,
//! not the speech itself: choosing `silent` here does not silence synthesis.
//! How loud they play is [`super::volume`], a separate setting because a user
//! who wants quieter cues usually does not want different ones.
//!
//! Three paths sit in this one file because they are one subject and each is
//! small: what is selected, what may be selected, and playing the selection
//! once so the user can hear it.

use crate::daemon::client::internal::response::{require_message, require_success};
use crate::daemon::client::internal::session::with_settings_token;
use crate::state::AudioTheme;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;

/// The themes on offer, falling back to the built-in set when the daemon
/// cannot be reached.
///
/// The fallback is what keeps the Customization page usable while the daemon is
/// down: the picker still renders, and the user's existing choice still shows,
/// where an empty dropdown would read as "this build has no themes".
pub async fn load_audio_themes() -> Vec<AudioTheme> {
    list_available_audio_themes()
        .await
        .unwrap_or_else(|_| AudioTheme::all_themes())
}

/// List available audio themes (HTTP `GET /settings/audio_theme/list`).
///
/// Read from the daemon rather than hard-coded here so a build that adds a
/// theme does not need the app rebuilt to offer it. A token the app does not
/// recognize is dropped rather than failing the list — an unknown theme is one
/// this app is too old to render a label for, not a broken response.
pub async fn list_available_audio_themes() -> HttpResult<Vec<AudioTheme>> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/settings/audio_theme/list").await?,
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

/// Read the configured audio-cue theme (HTTP `GET /settings/audio_theme`).
///
/// `silent` is a real selection, not an absent one: it means the user chose to
/// hear no cues, which is a different state from the volume being turned down.
pub async fn get_current_audio_theme() -> HttpResult<AudioTheme> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/settings/audio_theme").await?,
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

/// Select a theme without playing it (HTTP `POST /settings/audio_theme`).
///
/// Returns the daemon's confirmation text, which the Customization page shows
/// as its save feedback. An unknown token is rejected by the daemon rather than
/// quietly falling back to the default, so a typo cannot look like success.
pub async fn set_audio_theme(theme: AudioTheme) -> HttpResult<String> {
    let theme_str = theme.to_string().to_lowercase();
    with_settings_token(move |socket, token| {
        let theme_str = theme_str.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                "/settings/audio_theme",
                &serde_json::json!({ "theme": theme_str }),
            )
            .await?;
            require_message(resp, "set_theme")
        }
    })
    .await
}

/// Select a theme and audition it
/// (`POST /settings/audio_theme` then `POST /settings/audio_theme/test`).
///
/// Two requests, in that order, because the test plays the *selected* theme:
/// auditioning first would play the old one. The sound comes out of the host
/// the daemon runs on, so a remote caller gets a success nobody hears.
pub async fn set_and_test_audio_theme(theme: AudioTheme) -> HttpResult<String> {
    set_audio_theme(theme).await?;
    with_settings_token(|socket, token| async move {
        let resp = transport::settings_post(
            socket,
            &token,
            "/settings/audio_theme/test",
            &serde_json::json!({}),
        )
        .await?;
        require_message(resp, "test_theme")
    })
    .await
}
