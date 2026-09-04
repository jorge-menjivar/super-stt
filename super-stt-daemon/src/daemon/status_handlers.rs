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

    /// The models a pipeline stage can run, as `(name, source)` pairs.
    ///
    /// Scoped twice over: to the backend filling that stage, and to the models
    /// that carry the stage's role. Both matter. Offering a stage a model from
    /// another backend gives the user a pick that cannot load; offering it a
    /// model with the wrong role gives one that loads and then fails on every
    /// use — a post-processor selected as a transcription model fails each
    /// recording, silently, at the point the user has already spoken.
    ///
    /// This is the per-stage read. The full catalog with roles, across every
    /// installed backend, is `GET /backends`.
    pub async fn handle_list_stage_models(&self, post_processor: bool) -> DaemonResponse {
        let backends = self.backends.read().await;

        // The two stages name their backend differently: stage 1 holds an
        // install directory, stage 2 the `source` itself. Resolving each to the
        // same catalog entry is the only difference between them here.
        let backend = if post_processor {
            let source = self.config.read().await.post_processor.source.clone();
            (!source.is_empty()).then(|| backends.iter().find(|b| b.source == source))
        } else {
            self.active_backend.read().await.clone().map(|dir| {
                backends
                    .iter()
                    .find(|b| backends::dir_name(b).as_deref() == Some(dir.as_str()))
            })
        }
        .flatten();

        let available_models = backend
            .map(|b| {
                b.models
                    .iter()
                    .filter(|d| d.is_post_processor() == post_processor)
                    .map(|d| (d.name.clone(), d.source.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        info!(
            "Model list requested for stage {}, returning {} model(s)",
            u8::from(post_processor) + 1,
            available_models.len()
        );

        DaemonResponse::success()
            .with_available_models(available_models)
            .with_message("Available models listed successfully".to_string())
    }
}
