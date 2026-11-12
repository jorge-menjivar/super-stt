// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::device_management::PipelineStage;
use crate::daemon::types::{LoadedModel, SuperSTTDaemon};
use log::{info, warn};
use super_engine_daemon::stage::{Load, StageExt, Unload};
use super_voice_shared::models::protocol::{DaemonResponse, DaemonStatusEvent};

impl SuperSTTDaemon {
    /// Re-instantiate the currently-loaded model in place (same identity) so a
    /// changed secret or option takes effect. No-op when idle. Rejected during
    /// an active recording. A real-time (WebSocket) session holds the `model`
    /// read lock, so the reload waits for it.
    pub async fn handle_reload_active_model(&self) -> DaemonResponse {
        if let Some(resp) = self.guard_model_mutation("reload the model") {
            return resp;
        }
        let reloaded = self.stage(PipelineStage::Transcription).reload().await;
        if matches!(reloaded, Load::Unchanged) {
            return DaemonResponse::success().with_message("No active model to reload".to_string());
        }
        info!("Reloaded the active model to apply configuration changes");
        super::load_response("reload", reloaded)
    }

    /// Announce that `stage` has begun loading `model`. Every stage says which
    /// it is, because a client watching one stage must not read the other's
    /// load as its own — and these events are how it learns about a load it
    /// did not start (the daemon's startup load, or another client's).
    pub fn broadcast_model_loading_status(&self, model: &str, stage: PipelineStage) {
        self.events
            .publish_daemon_status(DaemonStatusEvent::LoadingModel {
                new_model: model.to_string(),
                stage: stage.position(),
            });
    }

    /// Announce that `model` is now the active, loaded model. Emits the
    /// `model_switched` event — which carries the full identity clients use to
    /// update their "current model" view — followed by the operational `ready`
    /// event. Every load of every stage announces itself this way, so clients
    /// see the same state whoever asked for it.
    ///
    /// `source` is included on the wire because a client reconnecting after a
    /// daemon restart has no prior `current_source` to fall back to; without it
    /// the model loads but the settings app keeps showing "no model loaded".
    pub fn broadcast_model_active(
        &self,
        name: &str,
        source: &str,
        actual_device: &str,
        stage: PipelineStage,
    ) {
        self.events
            .publish_daemon_status(DaemonStatusEvent::ModelSwitched {
                model_name: name.to_string(),
                source: source.to_string(),
                actual_device: actual_device.to_string(),
                stage: stage.position(),
            });
        self.events.publish_daemon_status(DaemonStatusEvent::Ready {
            model_loaded: true,
            model_name: Some(name.to_string()),
            actual_device: Some(actual_device.to_string()),
            preferred_device: None,
            stage: stage.position(),
        });
    }

    /// Unload the transcription model, after any load in flight: see
    /// [`ModelSlot::unload`](super_engine_daemon::model_slot::ModelSlot::unload).
    /// The stage stays as it is: for a change of backend, which records its
    /// own state.
    ///
    /// Hold what this returns while recording the change (config, events): no
    /// load starts until it is dropped. `None` when a load asked for since
    /// replaces the model anyway.
    #[must_use = "dropping it lets a load start before the unload is recorded"]
    pub(in crate::daemon) async fn unload_current_model(
        &self,
    ) -> Option<super_engine_daemon::model_slot::Unloaded<'_, LoadedModel>> {
        let unloaded = self.model.unload().await?;
        if unloaded.model.is_some() {
            info!("Current model unloaded");
        }
        Some(unloaded)
    }

    /// Unconditional unload used by the daemon's shutdown path, for every
    /// stage of both features. Bypasses the recording/realtime guard (the
    /// process is about to exit anyway) and gives each loaded model's backend
    /// a chance to stop while the runtime is still alive —
    /// `std::process::exit` later in `daemon_main` would otherwise skip every
    /// destructor, leaving each `systemd-run --user` subprocess unit orphaned.
    ///
    /// Waits for an idle unload still stopping its backend, which has the
    /// model out of its slot: see
    /// [`ModelSlot::shutdown`](super_engine_daemon::model_slot::ModelSlot::shutdown).
    pub async fn shutdown_unload(&self) {
        self.all_stages().shutdown().await;
    }

    /// Drop the currently loaded model and switch stage 1 off, so a restart
    /// or a request does not load it again. The selection stays, so loading
    /// it again is one click rather than a re-pick. No-op when no model is
    /// loaded and the stage is off. Rejected during an active recording /
    /// real-time session.
    ///
    /// A load in flight finishes first, and its model is the one unloaded;
    /// loads still waiting give up.
    pub async fn handle_unload_active_model(&self) -> DaemonResponse {
        if let Some(resp) = self.switch_guard() {
            return resp;
        }
        let message = match self.stage(PipelineStage::Transcription).unload().await {
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
