// SPDX-License-Identifier: GPL-3.0-only

use crate::config::DaemonConfig;
use crate::daemon::types::SuperSTTDaemon;
use crate::output::keyboard::Simulator;
use log::{info, warn};
use super_stt_shared::models::notification_method::NotificationMethod;
use super_stt_shared::models::protocol::{DaemonResponse, ErrorCode};
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use super_stt_shared::models::update_beta_optin::UpdateBetaOptIn;
use super_stt_shared::models::write_method::WriteMethod;

impl SuperSTTDaemon {
    /// What `POST /write_method/test` types. Fixed and documented in
    /// `docs/protocol/endpoints/v1/write_method/test.md`, so a client can tell
    /// the user what to expect; kept ASCII so a pass means the common case
    /// works rather than exercising high-keysym paths a backend may not map.
    const WRITE_METHOD_TEST_TEXT: &str = "Super STT input test 123";

    /// Mutate the config under the write lock, then persist it. Centralizes the
    /// lock → mutate → persist sequence so a settings handler can't hand-roll it
    /// and forget the persist (see Tier 1 #3). Returns the persist outcome so the
    /// caller can fold a save failure into its response.
    pub(in crate::daemon) async fn set_config_field<F>(&self, mutate: F) -> anyhow::Result<()>
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
    pub(in crate::daemon) fn settings_saved(
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

    /// Handle test write method command: type a fixed string with the
    /// configured method so the user can see whether it reaches their focused
    /// window. Contract: `docs/protocol/endpoints/v1/write_method/test.md`.
    pub async fn handle_test_write_method(&self) -> DaemonResponse {
        if *self.busy.read().await {
            return DaemonResponse::error_with_code(
                ErrorCode::RecordingInProgress,
                "recording_in_progress",
            );
        }

        let method = self.config.read().await.transcription.write_method;

        // Borrow the cached simulator rather than building a second one: a
        // fresh portal session costs three D-Bus round-trips and may re-prompt
        // for authorization, and the test would leave that session behind.
        let cached = self.simulator.write().await.take();
        let mut simulator = match cached {
            Some(s) => s,
            None => match Simulator::new(method).await {
                Ok(s) => s,
                Err(e) => {
                    warn!("Write-method test could not build a simulator: {e}");
                    return DaemonResponse::error_with_code(
                        ErrorCode::Internal,
                        "write_method_unavailable",
                    );
                }
            },
        };

        let resolved = simulator.resolved_method();
        let result = simulator.type_text(Self::WRITE_METHOD_TEST_TEXT).await;

        // Same cache discipline as a recording (see `Simulator::is_cacheable`).
        if simulator.is_cacheable() {
            *self.simulator.write().await = Some(simulator);
        }

        match result {
            Ok(()) => {
                info!("Write-method test typed via {resolved}");
                DaemonResponse::success()
                    .with_message(format!("Typed test text via {}", resolved.pretty_name()))
                    .with_write_method(method.to_string())
                    .with_resolved_write_method(resolved.to_string())
            }
            Err(e) => {
                warn!("Write-method test failed to type via {resolved}: {e}");
                DaemonResponse::error_with_code(ErrorCode::Internal, "typing_failed")
            }
        }
    }

    /// Handle get write method command
    pub async fn handle_get_write_method(&self) -> DaemonResponse {
        let config = self.config.read().await;
        let method = config.transcription.write_method;
        DaemonResponse::success().with_write_method(method.to_string())
    }

    /// Handle set notification method command. An unknown method name is
    /// rejected with `invalid_notification_method` (HTTP 400) per
    /// `docs/protocol/endpoints/v1/notification_method.md`, rather than
    /// silently applying the default and reporting success.
    pub async fn handle_set_notification_method(&self, method_str: String) -> DaemonResponse {
        let Ok(method) = method_str.parse::<NotificationMethod>() else {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidValue,
                "invalid_notification_method",
            );
        };

        let persist = self
            .set_config_field(|c| c.transcription.notification_method = method)
            .await;

        info!("Notification method set to {method}");
        Self::settings_saved(
            DaemonResponse::success().with_notification_method(method.to_string()),
            format!("Notification method set to {method}"),
            persist,
        )
    }

    /// Handle get notification method command
    pub async fn handle_get_notification_method(&self) -> DaemonResponse {
        let config = self.config.read().await;
        let method = config.transcription.notification_method;
        DaemonResponse::success().with_notification_method(method.to_string())
    }

    /// Handle set update-check-enabled command
    pub async fn handle_set_update_check_enabled(&self, enabled: bool) -> DaemonResponse {
        let persist = self
            .set_config_field(|c| c.update.check_enabled = enabled)
            .await;
        self.publish_settings_changed("update_check_enabled");
        info!(
            "Update checks {}",
            if enabled { "enabled" } else { "disabled" }
        );
        Self::settings_saved(
            DaemonResponse::success().with_update_check_enabled(enabled),
            format!(
                "Update checks {}",
                if enabled { "enabled" } else { "disabled" }
            ),
            persist,
        )
    }

    /// Handle get update-check-enabled command
    pub async fn handle_get_update_check_enabled(&self) -> DaemonResponse {
        let enabled = self.config.read().await.update.check_enabled;
        DaemonResponse::success().with_update_check_enabled(enabled)
    }

    /// Handle set update-beta-optin command. An unknown value is rejected
    /// with `invalid_update_beta_optin` (HTTP 400) per
    /// `docs/protocol/endpoints/v1/update_beta_optin.md`, rather than
    /// silently applying the default and reporting success.
    pub async fn handle_set_update_beta_optin(&self, value: String) -> DaemonResponse {
        let Ok(optin) = value.parse::<UpdateBetaOptIn>() else {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidValue,
                "invalid_update_beta_optin",
            );
        };
        let persist = self.set_config_field(|c| c.update.beta_optin = optin).await;
        self.publish_settings_changed("update_beta_optin");
        info!("Update beta opt-in set to {optin}");
        Self::settings_saved(
            DaemonResponse::success().with_update_beta_optin(optin.to_string()),
            format!("Update beta opt-in set to {optin}"),
            persist,
        )
    }

    /// Handle get update-beta-optin command
    pub async fn handle_get_update_beta_optin(&self) -> DaemonResponse {
        let optin = self.config.read().await.update.beta_optin;
        DaemonResponse::success().with_update_beta_optin(optin.to_string())
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
