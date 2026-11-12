// SPDX-License-Identifier: GPL-3.0-only
//! Text to speech's one pipeline stage, synthesis, as the engine's
//! [`Stage`](super_engine_daemon::stage::Stage) sees it: what it runs, how its
//! model is built, and how it tells clients. Every load and unload of its
//! model is the engine's
//! ([`StageExt`](super_engine_daemon::stage::StageExt)), the same as for
//! speech to text's stages.

use crate::tts::daemon::types::{LoadTurn, LoadedModel, SuperTTSDaemon, normalize_device};
use log::warn;
use std::future::Future;
use std::time::Duration;
use super_engine_daemon::model_slot::ModelSlot;
use super_engine_daemon::stage::{ModelId, Stage, StageEvent, StageModel};
use super_voice_shared::tts::models::protocol::DaemonStatusEvent;

impl StageModel for LoadedModel {
    fn id(&self) -> ModelId {
        ModelId::new(self.definition.name.clone(), self.definition.source.clone())
    }

    fn device(&self) -> String {
        normalize_device(&self.instance.device())
    }
}

/// The synthesis stage.
#[derive(Clone)]
pub(crate) struct TtsStage(SuperTTSDaemon);

impl SuperTTSDaemon {
    /// The synthesis stage, for its loads and unloads.
    pub(crate) fn stage(&self) -> TtsStage {
        TtsStage(self.clone())
    }

    /// Announce that the synthesis stage runs no model.
    fn announce_stage_empty(&self) {
        self.events
            .publish_synthesis_status(DaemonStatusEvent::Ready {
                model_loaded: false,
                model_name: None,
                actual_device: None,
                preferred_device: None,
            });
    }
}

impl Stage for TtsStage {
    type Model = LoadedModel;

    fn slot(&self) -> &ModelSlot<LoadedModel> {
        &self.0.model
    }

    async fn selected(&self) -> Result<ModelId, Option<String>> {
        let (name, source) = self.0.selected_model().await?;
        Ok(ModelId::new(name, source))
    }

    async fn instantiate(
        &self,
        turn: &LoadTurn<'_>,
        model: &ModelId,
        device: Option<&str>,
    ) -> anyhow::Result<LoadedModel> {
        let chosen = match device {
            Some(device) => Some(device.to_string()),
            None => self
                .0
                .config
                .read()
                .await
                .model_device(&model.source, &model.name)
                .map(ToString::to_string),
        };
        let (instance, definition) = self
            .0
            .instantiate_backend(turn, &model.name, &model.source, chosen.as_deref())
            .await?;
        if self.0.adopt_active_backend_for(&model.source).await
            && let Err(e) = self.0.persist_config().await
        {
            warn!("Failed to persist the active backend after a load: {e}");
        }
        Ok(LoadedModel::new(definition, instance))
    }

    async fn idle_after(&self) -> Option<Duration> {
        self.0.config.read().await.idle_unload.after()
    }

    /// An utterance being synthesized or played.
    fn in_use(&self) -> bool {
        self.0.is_busy()
    }

    fn announce(&self, event: StageEvent<'_>) {
        let daemon = &self.0;
        match event {
            StageEvent::Loading { model } => daemon.broadcast_model_loading_status(&model.name),
            StageEvent::Loaded { model, device } => {
                daemon.broadcast_model_active(&model.name, &model.source, device);
            }
            StageEvent::Unloaded => daemon.announce_stage_empty(),
            StageEvent::LoadFailed { .. } => {
                daemon.download_manager.clear_download(());
                daemon.announce_stage_empty();
            }
            StageEvent::SwitchingDevice { model, from, to } => {
                daemon
                    .events
                    .publish_synthesis_status(DaemonStatusEvent::SwitchingDevice {
                        from_device: from.to_string(),
                        target_device: to.to_string(),
                        model: model.name.clone(),
                    });
            }
            StageEvent::LoadingOnDevice { model, device } => {
                daemon
                    .events
                    .publish_synthesis_status(DaemonStatusEvent::LoadingModelForDevice {
                        model: model.name.clone(),
                        target_device: device.to_string(),
                    });
            }
            StageEvent::DeviceSwitchFailed {
                model,
                device,
                error,
            } => {
                daemon
                    .events
                    .publish_synthesis_status(DaemonStatusEvent::DeviceSwitchError {
                        error: error.to_string(),
                        failed_device: device.to_string(),
                        model: model.name.clone(),
                    });
            }
            StageEvent::DeviceSwitched {
                model,
                device,
                preferred,
            } => {
                daemon
                    .events
                    .publish_synthesis_status(DaemonStatusEvent::Ready {
                        model_loaded: true,
                        model_name: Some(model.name.clone()),
                        actual_device: Some(device.to_string()),
                        preferred_device: Some(preferred.to_string()),
                    });
            }
        }
    }

    async fn select(&self, model: &ModelId) {
        self.0
            .config
            .write()
            .await
            .update_synthesis_model(model.name.clone(), model.source.clone());
        if let Err(e) = self.0.persist_config().await {
            warn!("Failed to persist the selection of {model}: {e}");
        }
    }

    /// The synthesis stage has no switch of its own: its selection is what
    /// says it runs, so switching it off clears it.
    async fn switch_off(&self) -> bool {
        let was_on = {
            let mut config = self.0.config.write().await;
            let was_on = !config.synthesis.preferred_model.is_empty();
            config.clear_synthesis_model();
            was_on
        };
        // Persisted, so a restart stays idle rather than load the model again.
        if was_on && let Err(e) = self.0.persist_config().await {
            warn!("Failed to persist unloading the synthesis model: {e}");
        }
        was_on
    }

    async fn set_device(&self, model: &ModelId, device: &str) {
        self.0.config.write().await.update_model_device(
            &model.source,
            &model.name,
            Some(device.to_string()),
        );
        if let Err(e) = self.0.persist_config().await {
            warn!("Failed to persist the device of {model}: {e}");
        }
    }

    fn shutdown_signal(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut shutdown = self.0.shutdown_tx.subscribe();
        async move {
            let _ = shutdown.recv().await;
        }
    }
}
