// SPDX-License-Identifier: GPL-3.0-only

use crate::config::DaemonConfig;
use crate::daemon::types::SuperSTTDaemon;
use log::{info, warn};
use super_stt_shared::models::protocol::DaemonResponse;
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use super_stt_shared::models::write_method::WriteMethod;

impl SuperSTTDaemon {
    /// Mutate the config under the write lock, then persist it. Centralizes the
    /// lock → mutate → persist sequence so a settings handler can't hand-roll it
    /// and forget the persist (see Tier 1 #3). Returns the persist outcome so the
    /// caller can fold a save failure into its response.
    async fn set_config_field<F>(&self, mutate: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut DaemonConfig),
    {
        {
            let mut config = self.config.write().await;
            mutate(&mut config);
        }
        self.persist_config().await
    }

    /// Fold a persist outcome into a settings response. `base` already carries
    /// the setting-specific `.with_*` field and `message` is the success text.
    /// A save failure keeps the (already-applied) in-memory change — the daemon
    /// stays authoritative for the process lifetime — and appends the error to
    /// the message, logging a warning.
    fn settings_saved(
        base: DaemonResponse,
        message: String,
        persist: anyhow::Result<()>,
    ) -> DaemonResponse {
        match persist {
            Ok(()) => base.with_message(message),
            Err(e) => {
                warn!("Setting changed but config save failed: {e}");
                base.with_message(format!("{message} (save failed: {e})"))
            }
        }
    }
    /// Handle set preview typing command - enable or disable preview typing
    #[must_use]
    pub async fn handle_set_preview_typing(&self, enabled: bool) -> DaemonResponse {
        // Update the in-memory setting.
        self.preview_typing_enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);

        let persist = self
            .set_config_field(|c| c.transcription.preview_typing_enabled = enabled)
            .await;

        let state = if enabled { "enabled" } else { "disabled" };
        info!("Preview typing {state}");
        Self::settings_saved(
            DaemonResponse::success().with_preview_typing_enabled(enabled),
            format!("Preview typing {state}"),
            persist,
        )
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
        let persist = self
            .set_config_field(|c| c.transcription.recording_stop_mode = mode)
            .await;

        info!("Recording stop mode set to {mode}");
        Self::settings_saved(
            DaemonResponse::success().with_recording_stop_mode(mode.to_string()),
            format!("Recording stop mode set to {mode}"),
            persist,
        )
    }

    /// Handle get recording stop mode command
    pub async fn handle_get_recording_stop_mode(&self) -> DaemonResponse {
        let config = self.config.read().await;
        let mode = config.transcription.recording_stop_mode;
        DaemonResponse::success().with_recording_stop_mode(mode.to_string())
    }

    /// Handle set write method command
    pub async fn handle_set_write_method(&self, method: WriteMethod) -> DaemonResponse {
        let persist = self
            .set_config_field(|c| c.transcription.write_method = method)
            .await;
        // Invalidate the cached simulator so the next recording creates a fresh one.
        *self.simulator.write().await = None;

        info!("Write method set to {method}");
        Self::settings_saved(
            DaemonResponse::success().with_write_method(method.to_string()),
            format!("Write method set to {method}"),
            persist,
        )
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

        // If disabling online models and the current model is online, revert to a
        // local one. Track why the revert didn't leave a usable local model so the
        // response doesn't falsely claim "all transcription is local".
        let mut revert_warning: Option<String> = None;
        if !enabled {
            let current_is_online = {
                let guard = self.model.read().await;
                guard
                    .as_ref()
                    .is_some_and(|loaded| loaded.definition.is_online())
            };
            if current_is_online {
                info!("Online models disabled; reverting to a local model");
                if let Some((name, source)) = self.first_local_model().await {
                    let resp = self.handle_set_model(name, source).await;
                    if resp.status != "success" {
                        warn!("Reverting to a local model failed; unloading the online model");
                        *self.model.write().await = None;
                        revert_warning = Some(format!(
                            "could not load a local model: {}",
                            resp.message.unwrap_or_else(|| "unknown error".to_string())
                        ));
                    }
                } else {
                    warn!("No local backend installed to revert to; unloading current model");
                    *self.model.write().await = None;
                    revert_warning = Some("no local backend installed to fall back to".to_string());
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
                let message = if enabled {
                    "Online models enabled — audio may be sent to external APIs".to_string()
                } else {
                    match &revert_warning {
                        Some(w) => format!("Online models disabled, but {w}"),
                        None => "Online models disabled — all transcription is local".to_string(),
                    }
                };
                DaemonResponse::success()
                    .with_allow_online_models(enabled)
                    .with_message(message)
            }
            Err(e) => {
                warn!("Online models setting changed but config save failed: {e}");
                let mut message = format!(
                    "Online models {} (config save failed: {e})",
                    if enabled { "enabled" } else { "disabled" }
                );
                if let Some(w) = &revert_warning {
                    message = format!("{message}; {w}");
                }
                DaemonResponse::success()
                    .with_allow_online_models(enabled)
                    .with_message(message)
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

        let persist = self
            .set_config_field(|c| c.transcription.custom_models_dir = path)
            .await;

        info!("Custom models directory set to {path_display}");
        Self::settings_saved(
            DaemonResponse::success(),
            format!("Custom models directory set to {path_display}"),
            persist,
        )
    }
}
