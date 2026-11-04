// SPDX-License-Identifier: GPL-3.0-only

use crate::config::DaemonConfig;
use crate::daemon::types::SuperTTSDaemon;
use log::{info, warn};
use super_tts_shared::models::notification_method::NotificationMethod;
use super_tts_shared::models::protocol::{DaemonResponse, ErrorCode};
use super_tts_shared::models::update_beta_optin::UpdateBetaOptIn;

impl SuperTTSDaemon {
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
            .set_config_field(|c| c.synthesis.notification_method = method)
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
        let method = config.synthesis.notification_method;
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
}
