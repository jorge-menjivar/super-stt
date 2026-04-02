// SPDX-License-Identifier: GPL-3.0-only

use crate::audio::beeper::play_beep_sequence;
use crate::daemon::types::SuperSTTDaemon;
use log::{error, info};
use std::sync::Arc;
use super_stt_shared::models::protocol::DaemonResponse;
use super_stt_shared::theme::AudioTheme;

impl SuperSTTDaemon {
    /// Handle set audio theme command
    #[must_use]
    pub fn handle_set_audio_theme(&self, theme_str: String) -> DaemonResponse {
        let theme = theme_str.parse::<AudioTheme>().unwrap_or_default();
        self.set_audio_theme(theme);

        // Update the config with new audio theme and save to disk
        // Note: This needs to be done in an async context, but this method is sync
        // We'll spawn a task to handle the config update and broadcast
        let config_clone = Arc::clone(&self.config);
        let notification_manager = Arc::clone(&self.notification_manager);
        tokio::spawn(async move {
            let mut config_guard = config_clone.write().await;
            config_guard.update_audio_theme(theme);
            drop(config_guard);

            // Broadcast config change event
            if let Err(e) =
                SuperSTTDaemon::broadcast_config_change_static(&notification_manager, &config_clone)
                    .await
            {
                log::warn!("Failed to broadcast config change after audio theme change: {e}");
            }
        });

        DaemonResponse::success()
            .with_message(format!("Audio theme set to: {theme}",))
            .with_audio_theme(theme_str)
    }

    /// Handle get audio theme command
    #[must_use]
    pub fn handle_get_audio_theme(&self) -> DaemonResponse {
        let current_theme = self.get_audio_theme();
        DaemonResponse::success()
            .with_audio_theme(current_theme.to_string())
            .with_message(format!("Current theme: {current_theme}",))
    }

    /// Handle set volume command
    #[must_use]
    pub fn handle_set_volume(&self, volume: u8) -> DaemonResponse {
        self.set_volume(volume);

        let config_clone = Arc::clone(&self.config);
        let notification_manager = Arc::clone(&self.notification_manager);
        tokio::spawn(async move {
            let mut config_guard = config_clone.write().await;
            config_guard.update_volume(volume);
            drop(config_guard);

            if let Err(e) =
                SuperSTTDaemon::broadcast_config_change_static(&notification_manager, &config_clone)
                    .await
            {
                log::warn!("Failed to broadcast config change after volume change: {e}");
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
        match play_beep_sequence(
            &start_frequencies,
            start_duration,
            start_fade_in,
            start_fade_out,
            volume,
        ) {
            Ok(()) => {
                info!("Start sound completed successfully");

                // Test end sound as well
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                info!("Playing end sound...");
                match play_beep_sequence(
                    &end_frequencies,
                    end_duration,
                    end_fade_in,
                    end_fade_out,
                    volume,
                ) {
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
