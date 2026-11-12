// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::device_management::PipelineStage;
use crate::daemon::types::{LoadTurn, LoadedModel, SuperSTTDaemon, normalize_device};
use crate::stt_models::ModelDefinition;
use crate::stt_models::transcribe::Transcribe;
use anyhow::Result;
use log::{error, info, warn};
use super_voice_shared::models::protocol::{DaemonResponse, DaemonStatusEvent};

impl SuperSTTDaemon {
    /// Load a model on an explicit device (used during device switching).
    ///
    /// `target_device` is the user's `cpu`/`gpu` preference; `instantiate_backend`
    /// — the single funnel every load path (startup, model switch, device
    /// switch, reload) goes through — resolves it into the concrete
    /// accelerator (`cpu`/`cuda`/`rocm`/`metal`/`vulkan`) a subprocess backend
    /// is actually handed on `POST /v1/load`, per
    /// `docs/protocol/backend/contract.md`.
    ///
    /// Returns the instance with the definition it was instantiated from, so
    /// the caller finalizes against the same lookup — the backend can be
    /// uninstalled concurrently, and a second lookup could come back empty.
    ///
    /// # Errors
    /// Returns an error if no backend serves the model or instantiation fails.
    pub async fn load_model_with_target_device(
        &self,
        turn: &LoadTurn<'_>,
        name: &str,
        source: &str,
        target_device: &str,
    ) -> Result<(Box<dyn Transcribe>, ModelDefinition)> {
        info!("Loading model {name} with target device: {target_device}");
        self.broadcast_device_model_loading_status(name, target_device);
        let (instance, definition) = self
            .instantiate_backend(
                turn,
                name,
                source,
                Some(target_device),
                PipelineStage::Transcription,
            )
            .await?;
        info!(
            "Model {name} loaded on {}",
            normalize_device(&instance.device())
        );
        Ok((instance, definition))
    }

    /// Re-instantiate the currently-loaded model in place (same identity) so a
    /// changed secret or option takes effect. No-op when idle. Rejected during
    /// an active recording. A real-time (WebSocket) session holds the `model`
    /// read lock, so the reload's write-lock acquisition serializes behind it.
    pub async fn handle_reload_active_model(&self) -> DaemonResponse {
        if let Some(resp) = self.guard_model_mutation("reload the model").await {
            return resp;
        }
        let current = self
            .model
            .read()
            .await
            .as_ref()
            .map(|l| (l.definition.name.clone(), l.definition.source.clone()));
        let Some((name, source)) = current else {
            return DaemonResponse::success().with_message("No active model to reload".to_string());
        };

        info!("Reloading active model {name} to apply configuration changes");
        self.broadcast_model_loading_status(&name, PipelineStage::Transcription);
        // Takes the model out once it is this load's turn.
        let Some(turn) = self.model.begin_load().await else {
            return super::superseded("reload", &name);
        };
        let chosen = self
            .config
            .read()
            .await
            .model_device(&source, &name)
            .map(ToString::to_string);
        match self
            .instantiate_backend(
                &turn,
                &name,
                &source,
                chosen.as_deref(),
                PipelineStage::Transcription,
            )
            .await
        {
            Ok((instance, definition)) => {
                self.finalize_model_switch_success(turn, name, source, definition, instance)
                    .await
            }
            Err(e) => {
                error!("Model reload failed: {e}");
                DaemonResponse::error(&format!("Model reload failed: {e}"))
            }
        }
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

    /// Broadcast model loading status specifically for device switching.
    ///
    /// Stage 1's, always: the switch-with-recovery machinery this belongs to is
    /// the transcription stage's. A post-processor's device change is a plain
    /// reload, and reports itself as one.
    pub fn broadcast_device_model_loading_status(&self, model: &str, target_device: &str) {
        self.events
            .publish_daemon_status(DaemonStatusEvent::LoadingModelForDevice {
                model: model.to_string(),
                target_device: target_device.to_string(),
                stage: PipelineStage::Transcription.position(),
            });
    }

    /// Announce that `model` is now the active, loaded model. Emits the
    /// `model_switched` event — which carries the full identity clients use to
    /// update their "current model" view — followed by the operational `ready`
    /// event. Used by both the startup load of the persisted model and
    /// user-initiated switches so the two paths broadcast identical state.
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
    /// The backend is stopped outside the slot's lock, since it may take
    /// seconds to free GPU memory.
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

    /// Install a freshly loaded model on its `turn` and return its normalized
    /// device label. Its idle time starts now. Shared by the model-switch and
    /// device-switch finalize paths.
    pub(in crate::daemon) async fn finalize_loaded_model(
        &self,
        turn: LoadTurn<'_>,
        definition: crate::stt_models::ModelDefinition,
        instance: Box<dyn Transcribe>,
    ) -> String {
        let actual_device = normalize_device(&instance.device());
        turn.install(LoadedModel {
            definition,
            instance,
        })
        .await;
        actual_device
    }

    /// Unconditional unload used by the daemon's shutdown path. Bypasses the
    /// recording/realtime guard (the process is about to exit anyway) and
    /// gives the loaded model's `Drop` impl a chance to run while the
    /// runtime is still alive — `std::process::exit` later in
    /// `daemon_main` would otherwise skip every destructor, leaving the
    /// `systemd-run --user` subprocess unit orphaned.
    ///
    /// Waits for an idle unload still stopping its backend, which has the
    /// model out of its slot: see
    /// [`ModelSlot::shutdown`](super_engine_daemon::model_slot::ModelSlot::shutdown).
    pub async fn shutdown_unload(&self) {
        if self.model.shutdown().await.is_some() {
            info!("Shutdown: unloaded the current model so its subprocess unit exits cleanly");
        }
        // The post-processor is a second backend instance with its own
        // subprocess unit, so it leaks exactly the same way if it is not
        // stopped here.
        if self.post_processor.shutdown().await.is_some() {
            info!("Shutdown: unloaded the post-processor so its subprocess unit exits cleanly");
        }
    }

    /// Drop the currently loaded model. The active backend stays selected
    /// (the user can immediately pick another of its models); to fully idle
    /// out, clear the active backend instead. No-op when no model is loaded
    /// and none is switched on. Rejected during an active recording /
    /// real-time session.
    ///
    /// A load in flight finishes first, and its model is the one unloaded;
    /// loads still waiting give up. The unload runs as its own task, so a
    /// client that stops waiting for it does not leave it half done.
    pub async fn handle_unload_active_model(&self) -> DaemonResponse {
        if let Some(resp) = self.switch_guard().await {
            return resp;
        }
        let bg = self.clone();
        tokio::spawn(async move { bg.unload_active_model().await })
            .await
            .unwrap_or_else(|e| DaemonResponse::error(&format!("Unloading the model failed: {e}")))
    }

    async fn unload_active_model(&self) -> DaemonResponse {
        let Some(unloaded) = self.unload_current_model().await else {
            return DaemonResponse::success()
                .with_message("A newer model load replaces the model".to_string());
        };
        let dropped = unloaded.model.as_ref().map(|l| l.definition.name.clone());
        // A model can be switched on without being in memory: unloaded for
        // being idle, or one whose load failed. The next request or start
        // would load it, so it is switched off too.
        if dropped.is_none() && !self.config.read().await.transcription.is_active() {
            return DaemonResponse::success().with_message("No model to unload".to_string());
        }
        // Switch the stage off *and persist* so a daemon restart stays idle
        // instead of reloading the just-unloaded model. The selection itself
        // stays: the card shows the model it had, and loading it again — onto
        // the same device or another — is one click rather than a re-pick.
        // Done while the slot is held, so no load on demand reads the stage
        // as still on.
        self.config.write().await.disable_transcription();
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after unloading model: {e}");
        }
        self.events.publish_daemon_status(DaemonStatusEvent::Ready {
            model_loaded: false,
            model_name: None,
            actual_device: None,
            preferred_device: None,
            stage: PipelineStage::Transcription.position(),
        });
        drop(unloaded);
        let msg = dropped.map_or_else(|| "Model unloaded".to_string(), |n| format!("Unloaded {n}"));
        info!("{msg}");
        DaemonResponse::success().with_message(msg)
    }
}
