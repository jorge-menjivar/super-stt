// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use log::{error, info, warn};
use super_stt_shared::models::protocol::DaemonResponse;
use super_stt_shared::models::provider::Provider;
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use super_stt_shared::models::registry;
use super_stt_shared::models::write_method::WriteMethod;
use super_stt_shared::theme::AudioTheme;

impl SuperSTTDaemon {
    /// Handle ping command — returns `pong`. The HTTP layer is the only
    /// caller now, so there's no per-client liveness map to consult; a
    /// successful HTTP response is itself the liveness signal.
    #[must_use]
    pub fn handle_ping(&self, _client_id: Option<String>) -> DaemonResponse {
        DaemonResponse::success().with_message("pong".to_string())
    }

    /// Handle status command - return daemon and model status
    pub async fn handle_status(&self) -> DaemonResponse {
        let model_guard = self.model.read().await;

        let (device, model_loaded, current_model) = match model_guard.as_ref() {
            Some(loaded) => {
                let device_str = match loaded.instance.device() {
                    candle_core::Device::Cpu => "cpu".to_string(),
                    candle_core::Device::Cuda(_) => "cuda".to_string(),
                    candle_core::Device::Metal(_) => "metal".to_string(),
                };
                (device_str, true, Some(loaded.definition.name.to_string()))
            }
            None => ("unknown".to_string(), false, None),
        };

        let is_recording = *self.is_recording.read().await;

        let mut response = DaemonResponse::success()
            .with_device(device)
            .with_model_loaded(model_loaded)
            .with_is_recording(is_recording);

        if let Some(model) = current_model {
            response = response.with_current_model(model);
        }

        response
    }

    /// Handle get config command - return current daemon configuration
    pub async fn handle_get_config(&self) -> DaemonResponse {
        let config = self.config.read().await;

        // Serialize the config to JSON Value for the response
        let config_json = match serde_json::to_value(&*config) {
            Ok(value) => value,
            Err(e) => {
                error!("Failed to serialize daemon config: {e}");
                return DaemonResponse::error(&format!("Failed to serialize config: {e}"));
            }
        };

        DaemonResponse::success()
            .with_daemon_config(config_json)
            .with_message("Daemon configuration retrieved successfully".to_string())
    }

    /// Handle list all available models command (built-in + custom)
    pub async fn handle_list_models(&self) -> DaemonResponse {
        let mut available_models: Vec<(
            String,
            super_stt_shared::models::provider::Provider,
            super_stt_shared::models::registry::SourceKind,
        )> = registry::ALL
            .iter()
            .map(|d| (d.name.to_string(), d.provider, d.source.kind()))
            .collect();

        // Append custom models
        let custom = self.custom_models.read().await;
        for cm in custom.iter() {
            available_models.push((
                cm.name.clone(),
                cm.provider,
                super_stt_shared::models::registry::SourceKind::Custom,
            ));
        }

        info!(
            "Available models requested, returning {} models ({} custom)",
            available_models.len(),
            custom.len()
        );

        DaemonResponse::success()
            .with_available_models(available_models)
            .with_message("Available models listed successfully".to_string())
    }

    /// Re-scan `custom_models_dir` and update the registry.
    pub async fn refresh_custom_models(&self) {
        let dir = self
            .config
            .read()
            .await
            .transcription
            .custom_models_dir
            .clone();

        let models = if let Some(dir) = dir {
            crate::stt_models::local::download::discover_custom_models(&std::path::PathBuf::from(
                dir,
            ))
        } else {
            Vec::new()
        };

        info!("Custom models registry: {} model(s)", models.len());
        *self.custom_models.write().await = models;
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

    /// Handle set preview typing command - enable or disable preview typing
    #[must_use]
    pub async fn handle_set_preview_typing(&self, enabled: bool) -> DaemonResponse {
        // Update the in-memory setting
        self.preview_typing_enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);

        // Update the config directly (don't clone)
        {
            let mut config_guard = self.config.write().await;
            config_guard.transcription.preview_typing_enabled = enabled;
        }

        // Persist the change to disk so it survives a restart.
        let persist_result = self.persist_config().await;

        match persist_result {
            Ok(()) => {
                info!(
                    "Preview typing {} and saved to config",
                    if enabled { "enabled" } else { "disabled" }
                );
                DaemonResponse::success()
                    .with_preview_typing_enabled(enabled)
                    .with_message(format!(
                        "Preview typing {} and saved",
                        if enabled { "enabled" } else { "disabled" }
                    ))
            }
            Err(e) => {
                warn!("Preview typing setting changed but failed to save to config: {e}");
                DaemonResponse::success()
                    .with_preview_typing_enabled(enabled)
                    .with_message(format!(
                        "Preview typing {} (config save failed: {e})",
                        if enabled { "enabled" } else { "disabled" }
                    ))
            }
        }
    }

    /// Handle get preview typing command - return current preview typing setting
    #[must_use]
    pub fn handle_get_preview_typing(&self) -> DaemonResponse {
        let enabled = self
            .preview_typing_enabled
            .load(std::sync::atomic::Ordering::Relaxed);

        DaemonResponse::success()
            .with_preview_typing_enabled(enabled)
            .with_message("Preview typing setting retrieved successfully".to_string())
    }

    /// Handle set recording stop mode command
    pub async fn handle_set_recording_stop_mode(&self, mode: RecordingStopMode) -> DaemonResponse {
        {
            let mut config_guard = self.config.write().await;
            config_guard.transcription.recording_stop_mode = mode;
        }

        let persist_result = self.persist_config().await;

        match persist_result {
            Ok(()) => {
                info!("Recording stop mode set to {mode} and saved to config");
                DaemonResponse::success()
                    .with_recording_stop_mode(mode.to_string())
                    .with_message(format!("Recording stop mode set to {mode}"))
            }
            Err(e) => {
                warn!("Recording stop mode changed but failed to save: {e}");
                DaemonResponse::success()
                    .with_recording_stop_mode(mode.to_string())
                    .with_message(format!(
                        "Recording stop mode set to {mode} (save failed: {e})"
                    ))
            }
        }
    }

    /// Handle get recording stop mode command
    pub async fn handle_get_recording_stop_mode(&self) -> DaemonResponse {
        let config = self.config.read().await;
        let mode = config.transcription.recording_stop_mode;
        DaemonResponse::success().with_recording_stop_mode(mode.to_string())
    }

    /// Handle set write method command
    pub async fn handle_set_write_method(&self, method: WriteMethod) -> DaemonResponse {
        {
            let mut config_guard = self.config.write().await;
            config_guard.transcription.write_method = method;
        }
        // Invalidate the cached simulator so the next recording creates a fresh one.
        *self.simulator.write().await = None;

        let persist_result = self.persist_config().await;

        match persist_result {
            Ok(()) => {
                info!("Write method set to {method} and saved to config");
                DaemonResponse::success()
                    .with_write_method(method.to_string())
                    .with_message(format!("Write method set to {method}"))
            }
            Err(e) => {
                warn!("Write method changed but failed to save: {e}");
                DaemonResponse::success()
                    .with_write_method(method.to_string())
                    .with_message(format!("Write method set to {method} (save failed: {e})"))
            }
        }
    }

    /// Handle get write method command
    pub async fn handle_get_write_method(&self) -> DaemonResponse {
        let config = self.config.read().await;
        let method = config.transcription.write_method;
        DaemonResponse::success().with_write_method(method.to_string())
    }

    /// Handle set allow online models command
    pub async fn handle_set_allow_online_models(&self, enabled: bool) -> DaemonResponse {
        {
            let mut config_guard = self.config.write().await;
            config_guard.online.allow_online_models = enabled;
        }

        // If disabling online models and current model is online, revert to default
        if !enabled {
            let current_is_online = {
                let guard = self.model.read().await;
                guard
                    .as_ref()
                    .is_some_and(|loaded| matches!(loaded.definition.provider, Provider::Online(_)))
            };
            if current_is_online {
                info!("Online models disabled; reverting to default local model");
                let default = registry::default_definition();
                let _ = self
                    .handle_set_model(
                        default.name.to_string(),
                        default.provider,
                        default.source.kind(),
                    )
                    .await;
            }
        }

        let persist_result = self.persist_config().await;

        match persist_result {
            Ok(()) => {
                info!(
                    "Online models {}",
                    if enabled { "enabled" } else { "disabled" }
                );
                DaemonResponse::success()
                    .with_allow_online_models(enabled)
                    .with_message(format!(
                        "Online models {}",
                        if enabled {
                            "enabled — audio may be sent to external APIs"
                        } else {
                            "disabled — all transcription is local"
                        }
                    ))
            }
            Err(e) => {
                warn!("Online models setting changed but config save failed: {e}");
                DaemonResponse::success()
                    .with_allow_online_models(enabled)
                    .with_message(format!(
                        "Online models {} (config save failed: {e})",
                        if enabled { "enabled" } else { "disabled" }
                    ))
            }
        }
    }

    /// Handle get allow online models command
    pub async fn handle_get_allow_online_models(&self) -> DaemonResponse {
        let config = self.config.read().await;
        let enabled = config.online.allow_online_models;
        DaemonResponse::success()
            .with_allow_online_models(enabled)
            .with_message("Allow online models setting retrieved".to_string())
    }

    /// Handle get custom models directory command
    pub async fn handle_get_custom_models_dir(&self) -> DaemonResponse {
        let path = self
            .config
            .read()
            .await
            .transcription
            .custom_models_dir
            .clone();
        DaemonResponse::success().with_custom_models_dir(path)
    }

    /// Handle set custom models directory command
    pub async fn handle_set_custom_models_dir(&self, path: Option<String>) -> DaemonResponse {
        let path_display = path.as_deref().unwrap_or("none").to_string();

        {
            let mut config_guard = self.config.write().await;
            config_guard.transcription.custom_models_dir = path;
        }

        // Re-scan the new directory
        self.refresh_custom_models().await;

        let persist_result = self.persist_config().await;

        match persist_result {
            Ok(()) => {
                info!("Custom models directory set to {path_display} and saved to config");
                DaemonResponse::success()
                    .with_message(format!("Custom models directory set to {path_display}"))
            }
            Err(e) => {
                warn!("Custom models directory changed but failed to save: {e}");
                DaemonResponse::success().with_message(format!(
                    "Custom models directory set to {path_display} (save failed: {e})"
                ))
            }
        }
    }

    /// Handle cancel download command
    #[must_use]
    pub fn handle_cancel_download(&self) -> DaemonResponse {
        match self.download_manager.cancel_current_download() {
            Ok(()) => {
                info!("Download cancellation requested");
                DaemonResponse::success()
                    .with_message("Download cancelled successfully".to_string())
            }
            Err(e) => {
                warn!("Failed to cancel download: {e}");
                DaemonResponse::error(&e)
            }
        }
    }

    /// Handle get download status command
    #[must_use]
    pub fn handle_get_download_status(&self) -> DaemonResponse {
        if let Some(tracker) = self.download_manager.get_current_download() {
            let progress = tracker.get_progress();
            DaemonResponse::success().with_download_progress(progress)
        } else {
            DaemonResponse::success().with_message("No download in progress".to_string())
        }
    }
}
