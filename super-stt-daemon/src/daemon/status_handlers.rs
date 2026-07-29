// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::backends;
use log::{error, info};
use super_stt_shared::models::protocol::DaemonResponse;

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
    /// installed backend, as `(name, source)` pairs.
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
                    .map(|d| (d.name.clone(), d.source.clone()))
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
}
