// SPDX-License-Identifier: GPL-3.0-only

use crate::audio::beeper::play_beep_sequence_async;
use crate::daemon::types::SuperSTTDaemon;
use log::{error, info};
use std::sync::Arc;
use super_stt_shared::models::protocol::{DaemonResponse, ErrorCode};
use super_stt_shared::theme::AudioTheme;

impl SuperSTTDaemon {
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

            if let Err(e) = SuperSTTDaemon::persist_config_static(&config_clone).await {
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

            if let Err(e) = SuperSTTDaemon::persist_config_static(&config_clone).await {
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

    /// Handle test audio theme command
    pub async fn handle_test_audio_theme(&self) -> DaemonResponse {
        let current_theme = self.get_audio_theme();
        let theme_name = format!("{current_theme:?}").to_lowercase();

        // Skip playing sounds for Silent theme
        if current_theme == AudioTheme::Silent {
            info!("Testing audio theme: {theme_name} (silent - no sounds played)");
            return DaemonResponse::success().with_message(
                "Audio theme 'Silent' tested successfully - no sounds played".to_string(),
            );
        }

        // Play both start and end sounds to test the theme
        let (start_frequencies, start_duration, start_fade_in, start_fade_out) =
            current_theme.start_sound();
        let (end_frequencies, end_duration, end_fade_in, end_fade_out) = current_theme.end_sound();

        let volume = self.get_volume_f32();
        info!(
            "Testing audio theme: {theme_name} (volume: {}%)",
            self.get_volume()
        );
        info!("Start frequencies: {start_frequencies:?}, duration: {start_duration}ms");
        info!("End frequencies: {end_frequencies:?}, duration: {end_duration}ms");

        // Test with start sound first
        info!("Playing start sound...");
        match play_beep_sequence_async(
            start_frequencies,
            start_duration,
            start_fade_in,
            start_fade_out,
            volume,
        )
        .await
        {
            Ok(()) => {
                info!("Start sound completed successfully");

                // Test end sound as well
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                info!("Playing end sound...");
                match play_beep_sequence_async(
                    end_frequencies,
                    end_duration,
                    end_fade_in,
                    end_fade_out,
                    volume,
                )
                .await
                {
                    Ok(()) => {
                        info!("End sound completed successfully");
                        DaemonResponse::success()
                            .with_message("Audio theme test completed successfully".to_string())
                    }
                    Err(e) => {
                        error!("Failed to play end sound: {e}");
                        DaemonResponse::success()
                            .with_message(format!("Audio theme tested, but end sound failed: {e}. This is likely due to audio access permissions."))
                    }
                }
            }
            Err(e) => {
                error!("Failed to play start sound: {e}");
                DaemonResponse::success()
                    .with_message(format!("Audio theme tested, but playback failed: {e}. This is likely due to audio access permissions. The daemon needs to be in the 'audio' group."))
            }
        }
    }
}
