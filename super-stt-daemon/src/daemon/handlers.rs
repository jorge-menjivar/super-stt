// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::backends;
use log::{error, info, warn};
use super_stt_shared::models::protocol::DaemonResponse;
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
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
                let device = crate::daemon::types::normalize_device(&loaded.instance.device());
                (device, true, Some(loaded.definition.name.clone()))
            }
            None => ("unknown".to_string(), false, None),
        };

        let busy = *self.busy.read().await;

        let mut response = DaemonResponse::success()
            .with_device(device)
            .with_model_loaded(model_loaded)
            .with_busy(busy);

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

    /// Handle list all available models command — every model served by an
    /// installed backend, as `(name, provider, source)` triples.
    pub async fn handle_list_models(&self) -> DaemonResponse {
        // Scoped to the active backend: only its models are switchable. The
        // full catalog of installed backends lives at `GET /backends`.
        let backends = self.backends.read().await;
        let active_dir = self.active_backend.read().await.clone();
        let available_models = active_dir
            .and_then(|dir| {
                backends
                    .iter()
                    .find(|b| backends::dir_name(b).as_deref() == Some(dir.as_str()))
            })
            .map(|b| {
                b.models
                    .iter()
                    .map(|d| (d.name.clone(), d.provider.clone(), d.source.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        info!(
            "Available models requested, returning {} model(s) for the active backend",
            available_models.len()
        );

        DaemonResponse::success()
            .with_available_models(available_models)
            .with_message("Available models listed successfully".to_string())
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
                    .is_some_and(|loaded| loaded.definition.is_online())
            };
            if current_is_online {
                info!("Online models disabled; reverting to a local model");
                if let Some((name, provider, source)) = self.first_local_model().await {
                    let _ = self.handle_set_model(name, provider, source).await;
                } else {
                    warn!("No local backend installed to revert to; unloading current model");
                    *self.model.write().await = None;
                }
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

    /// Handle list backends command — the installed-backend catalog with each
    /// backend's models, declared secrets, and options (with effective values).
    /// Drives the settings UI; see `docs/protocol/endpoints/v1/backends.md`.
    pub async fn handle_list_backends(&self) -> DaemonResponse {
        let config = self.config.read().await;
        let backends = self.backends.read().await;

        let catalog: Vec<serde_json::Value> = backends
            .iter()
            .map(|b| {
                let models: Vec<serde_json::Value> = b
                    .models
                    .iter()
                    .map(|m| {
                        serde_json::json!({
                            "name": m.name,
                            "provider": m.provider.to_string(),
                            "multilingual": m.is_multilingual,
                            "primary_language": m.primary_language,
                            "supported_languages": m.supported_languages,
                            "supported_devices": m.supported_devices,
                            "estimated_vram_bytes": m.estimated_vram_bytes,
                            "realtime": m.realtime,
                        })
                    })
                    .collect();
                let secrets: Vec<serde_json::Value> = b
                    .secrets
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "name": s.name,
                            "label": s.label,
                            "description": s.description,
                            "required": s.required,
                        })
                    })
                    .collect();
                let options: Vec<serde_json::Value> = b
                    .options
                    .iter()
                    .map(|o| {
                        let default = o.default.as_ref().map(ToString::to_string);
                        let value = config
                            .backend_option(&b.source, &o.name)
                            .map(str::to_string)
                            .or_else(|| default.clone());
                        serde_json::json!({
                            "name": o.name,
                            "label": o.label,
                            "description": o.description,
                            "type": o.r#type,
                            "default": default,
                            "required": o.required,
                            "value": value,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "source": b.source,
                    "name": b.name,
                    "kind": b.kind,
                    "allowed_hosts": b.allowed_hosts,
                    "models": models,
                    "secrets": secrets,
                    "options": options,
                })
            })
            .collect();

        info!("Backends catalog requested: {} backend(s)", catalog.len());
        DaemonResponse::success()
            .with_backends(serde_json::json!(catalog))
            .with_message("Backends listed successfully".to_string())
    }

    /// Reload the active model iff it is served by `source`, so a just-changed
    /// option/secret takes effect immediately. Shared by option and secret writes.
    async fn reload_if_source_active(&self, source: &str) {
        let active_source = self
            .model
            .read()
            .await
            .as_ref()
            .map(|l| l.definition.source.clone());
        if active_source.as_deref() == Some(source) {
            let _ = self.handle_reload_active_model().await;
        }
    }

    /// Handle set backend option command — store/clear a plaintext option
    /// override in config. Takes effect on the backend's next model load.
    pub async fn handle_set_backend_option(
        &self,
        source: String,
        name: String,
        value: String,
    ) -> DaemonResponse {
        {
            let mut config = self.config.write().await;
            config.update_backend_option(source.clone(), name.clone(), value.clone());
        }

        self.reload_if_source_active(&source).await;

        if value.is_empty() {
            info!("Cleared backend option {name} for {source}");
            DaemonResponse::success().with_message(format!("Option {name} cleared"))
        } else {
            info!("Set backend option {name} for {source}");
            DaemonResponse::success().with_message(format!("Option {name} updated"))
        }
    }

    /// Store (or replace) a backend secret and reload the active model if needed.
    pub async fn handle_set_backend_secret(
        &self,
        source: String,
        name: String,
        value: String,
    ) -> DaemonResponse {
        if let Err(e) = crate::keyring::set_backend_secret(&source, &name, &value) {
            return DaemonResponse::error(&format!("keyring_unavailable: {e}"));
        }
        self.reload_if_source_active(&source).await;
        info!("Set backend secret {name} for {source}");
        DaemonResponse::success().with_message(format!("Secret {name} stored"))
    }

    /// Clear a backend secret (reset to unset) and reload the active model if needed.
    pub async fn handle_clear_backend_secret(
        &self,
        source: String,
        name: String,
    ) -> DaemonResponse {
        if let Err(e) = crate::keyring::delete_backend_secret(&source, &name) {
            return DaemonResponse::error(&format!("keyring_unavailable: {e}"));
        }
        self.reload_if_source_active(&source).await;
        info!("Cleared backend secret {name} for {source}");
        DaemonResponse::success().with_message(format!("Secret {name} cleared"))
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
