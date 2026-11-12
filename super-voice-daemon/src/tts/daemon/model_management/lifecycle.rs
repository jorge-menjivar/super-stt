// SPDX-License-Identifier: GPL-3.0-only
use crate::tts::daemon::types::{LoadedModel, SuperTTSDaemon};
use log::{info, warn};
use super_engine_daemon::stage::{Load, StageExt, Unload};
use super_voice_shared::tts::models::protocol::{DaemonResponse, DaemonStatusEvent};

impl SuperTTSDaemon {
    /// Re-instantiate the currently-loaded model in place (same identity) so a
    /// changed secret or option takes effect. No-op when idle. Stops whatever
    /// is being spoken first — see [`stop_speech_for`](Self::stop_speech_for).
    pub async fn handle_reload_active_model(&self) -> DaemonResponse {
        if self.model.read().await.is_none() {
            return DaemonResponse::success().with_message("No active model to reload".to_string());
        }
        self.stop_speech_for("reload the model").await;
        let reloaded = self.stage().reload().await;
        if matches!(reloaded, Load::Unchanged) {
            return DaemonResponse::success().with_message("No active model to reload".to_string());
        }
        info!("Reloaded the active model to apply configuration changes");
        super::load_response("reload", reloaded)
    }

    pub fn broadcast_model_loading_status(&self, model: &str) {
        self.events
            .publish_synthesis_status(DaemonStatusEvent::LoadingModel {
                new_model: model.to_string(),
            });
    }

    /// Announce that `model` is now the active, loaded model. Emits the
    /// `model_switched` event — which carries the full identity clients use to
    /// update their "current model" view — followed by the operational `ready`
    /// event. Every load announces itself this way, so clients see the same
    /// state whoever asked for it.
    ///
    /// `source` is included on the wire because a client reconnecting after a
    /// daemon restart has no prior `current_source` to fall back to; without it
    /// the model loads but the settings app keeps showing "no model loaded".
    pub fn broadcast_model_active(&self, name: &str, source: &str, actual_device: &str) {
        self.events
            .publish_synthesis_status(DaemonStatusEvent::ModelSwitched {
                model_name: name.to_string(),
                source: source.to_string(),
                actual_device: actual_device.to_string(),
            });
        self.events
            .publish_synthesis_status(DaemonStatusEvent::Ready {
                model_loaded: true,
                model_name: Some(name.to_string()),
                actual_device: Some(actual_device.to_string()),
                preferred_device: None,
            });
    }

    /// Unload the synthesis model, after any load in flight: see
    /// [`ModelSlot::unload`](super_engine_daemon::model_slot::ModelSlot::unload).
    /// The selection stays as it is: for a change of backend, which records
    /// its own state.
    ///
    /// Hold what this returns while recording the change (config, events): no
    /// load starts until it is dropped. `None` when a load asked for since
    /// replaces the model anyway.
    #[must_use = "dropping it lets a load start before the unload is recorded"]
    pub(in crate::tts::daemon) async fn unload_current_model(
        &self,
    ) -> Option<super_engine_daemon::model_slot::Unloaded<'_, LoadedModel>> {
        let unloaded = self.model.unload().await?;
        if unloaded.model.is_some() {
            info!("Current model unloaded");
        }
        Some(unloaded)
    }

    /// Drop the currently loaded model and clear its selection, so a restart
    /// or a request does not load it again. The active backend stays selected
    /// (the user can immediately pick another of its models); to fully idle
    /// out, clear the active backend instead. No-op when no model is loaded
    /// and none is selected. Stops whatever is being spoken first — see
    /// [`stop_speech_for`](Self::stop_speech_for).
    ///
    /// A load in flight finishes first, and its model is the one unloaded;
    /// loads still waiting give up.
    pub async fn handle_unload_active_model(&self) -> DaemonResponse {
        self.stop_speech_for("unload the model").await;
        let message = match self.stage().unload().await {
            Unload::Unloaded { model: Some(model) } => format!("Unloaded {model}"),
            Unload::Unloaded { model: None } => "Model unloaded".to_string(),
            Unload::NothingToUnload => "No model to unload".to_string(),
            Unload::Superseded => "A newer model load replaces the model".to_string(),
            Unload::Cancelled => {
                warn!("Unloading the model stopped: the daemon is shutting down");
                return DaemonResponse::error(
                    "Unloading the model stopped: the daemon is shutting down",
                );
            }
        };
        DaemonResponse::success().with_message(message)
    }
}
