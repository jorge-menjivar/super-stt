// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperTTSDaemon;
use log::{error, info};
use std::sync::Arc;
use super_tts_shared::models::protocol::{DaemonResponse, ErrorCode};
use super_tts_shared::product::SUPER_TTS as PRODUCT;
use super_tts_shared::theme::AudioTheme;

impl SuperTTSDaemon {
    /// Handle set audio theme command. An unknown theme name is rejected with
    /// `invalid_audio_theme` (HTTP 400) per
    /// `docs/protocol/endpoints/v1/audio_theme.md`, rather than silently
    /// applying the default and reporting success.
    #[must_use]
    pub fn handle_set_audio_theme(&self, theme_str: String) -> DaemonResponse {
        // `AudioTheme::from_str` rejects an unrecognized token, so it validates
        // the input directly (no need to scan `all_themes`).
        let Ok(theme) = theme_str.parse::<AudioTheme>() else {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidAudioTheme,
                "invalid_audio_theme",
            );
        };
        self.set_audio_theme(theme);

        // Persist the change to disk so it survives a restart. The
        // handler is sync; spawn a task to mutate the config + flush.
        let config_clone = Arc::clone(&self.config);
        tokio::spawn(async move {
            let mut config_guard = config_clone.write().await;
            config_guard.update_audio_theme(theme);
            drop(config_guard);

            if let Err(e) = SuperTTSDaemon::persist_config_static(&config_clone).await {
                log::warn!("Failed to persist config after audio theme change: {e}");
            }
        });

        DaemonResponse::success()
            .with_message(format!("Audio theme set to: {theme}"))
            .with_audio_theme(theme_str)
    }

    /// Handle list audio themes command - return all available audio themes
    #[must_use]
    pub fn handle_list_audio_themes(&self) -> DaemonResponse {
        let available_themes = AudioTheme::all_themes();
        info!(
            "Available audio themes requested, returning {} themes",
            available_themes.len()
        );

        DaemonResponse::success()
            .with_available_audio_themes(available_themes)
            .with_message("Available audio themes listed successfully".to_string())
    }

    /// Handle get audio theme command
    #[must_use]
    pub fn handle_get_audio_theme(&self) -> DaemonResponse {
        let current_theme = self.get_audio_theme();
        DaemonResponse::success()
            .with_audio_theme(current_theme.to_string())
            .with_message(format!("Current theme: {current_theme}"))
    }

    /// Handle set volume command
    #[must_use]
    pub fn handle_set_volume(&self, volume: u8) -> DaemonResponse {
        self.set_volume(volume);

        let config_clone = Arc::clone(&self.config);
        tokio::spawn(async move {
            let mut config_guard = config_clone.write().await;
            config_guard.update_volume(volume);
            drop(config_guard);

            if let Err(e) = SuperTTSDaemon::persist_config_static(&config_clone).await {
                log::warn!("Failed to persist config after volume change: {e}");
            }
        });

        DaemonResponse::success().with_message(format!("Volume set to: {volume}"))
    }

    /// Handle get volume command
    #[must_use]
    pub fn handle_get_volume(&self) -> DaemonResponse {
        let volume = self.get_volume();
        DaemonResponse::success().with_message(format!("{volume}"))
    }

    /// Handle test audio theme command: play the selected theme's start and
    /// end cues once. See `super_engine_daemon::beeper::preview_theme`.
    pub async fn handle_test_audio_theme(&self) -> DaemonResponse {
        use super_engine_daemon::beeper::{PreviewError, preview_theme};

        let current_theme = self.get_audio_theme();
        if current_theme == AudioTheme::Silent {
            info!("Testing audio theme: silent (no sounds played)");
            return DaemonResponse::success().with_message(
                "Audio theme 'Silent' tested successfully - no sounds played".to_string(),
            );
        }
        info!(
            "Testing audio theme: {current_theme} (volume: {}%)",
            self.get_volume()
        );
        match preview_theme(&PRODUCT, current_theme, self.get_volume_f32()).await {
            Ok(()) => DaemonResponse::success()
                .with_message("Audio theme test completed successfully".to_string()),
            Err(PreviewError::Start(e)) => {
                error!("Failed to play start sound: {e}");
                DaemonResponse::success()
                    .with_message(format!("Audio theme tested, but playback failed: {e}. This is likely due to audio access permissions. The daemon needs to be in the 'audio' group."))
            }
            Err(PreviewError::End(e)) => {
                error!("Failed to play end sound: {e}");
                DaemonResponse::success()
                    .with_message(format!("Audio theme tested, but end sound failed: {e}. This is likely due to audio access permissions."))
            }
        }
    }
}
