// SPDX-License-Identifier: GPL-3.0-only
//! Speech to text's pipeline stages, as the engine's
//! [`Stage`](super_engine_daemon::stage::Stage) sees them: what each stage
//! runs, how its model is built, and how it tells clients. Every load and
//! unload of their models is the engine's
//! ([`StageExt`](super_engine_daemon::stage::StageExt)), the same for every
//! stage of both features.

use crate::daemon::device_management::PipelineStage;
use crate::daemon::types::{LoadTurn, LoadedModel, SuperSTTDaemon, normalize_device};
use log::warn;
use std::future::Future;
use std::time::Duration;
use super_engine_daemon::model_slot::ModelSlot;
use super_engine_daemon::stage::{ModelId, Stage, StageEvent, StageModel, Stages};
use super_voice_shared::models::protocol::DaemonStatusEvent;

impl StageModel for LoadedModel {
    fn id(&self) -> ModelId {
        ModelId::new(self.definition.name.clone(), self.definition.source.clone())
    }

    fn device(&self) -> String {
        normalize_device(&self.instance.device())
    }
}

/// One stage of speech to text's pipeline.
#[derive(Clone)]
pub(crate) struct SttStage {
    daemon: SuperSTTDaemon,
    stage: PipelineStage,
}

impl SuperSTTDaemon {
    /// `stage` of the pipeline, for its loads and unloads.
    pub(crate) fn stage(&self, stage: PipelineStage) -> SttStage {
        SttStage {
            daemon: self.clone(),
            stage,
        }
    }

    /// Every stage the daemon runs, of both features, for what is done to
    /// all of them at once: loading at startup, unloading when idle, and
    /// stopping at shutdown.
    pub(crate) fn all_stages(&self) -> Stages {
        PipelineStage::ALL
            .into_iter()
            .fold(Stages::new(), |stages, stage| {
                stages.with(self.stage(stage))
            })
            .with(self.tts.stage())
    }

    /// Announce that `stage` runs no model.
    pub(in crate::daemon) fn announce_stage_empty(&self, stage: PipelineStage) {
        self.events.publish_daemon_status(DaemonStatusEvent::Ready {
            model_loaded: false,
            model_name: None,
            actual_device: None,
            preferred_device: None,
            stage: stage.position(),
        });
    }
}

impl Stage for SttStage {
    type Model = LoadedModel;

    fn slot(&self) -> &ModelSlot<LoadedModel> {
        match self.stage {
            PipelineStage::Transcription => &self.daemon.model,
            PipelineStage::PostProcessor => &self.daemon.post_processor,
        }
    }

    async fn selected(&self) -> Result<ModelId, Option<String>> {
        let (name, source) = self.daemon.selected_model(self.stage).await?;
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
                .daemon
                .config
                .read()
                .await
                .model_device(&model.source, &model.name)
                .map(ToString::to_string),
        };
        let (instance, definition) = self
            .daemon
            .instantiate_backend(
                turn,
                &model.name,
                &model.source,
                chosen.as_deref(),
                self.stage,
            )
            .await?;
        if self.stage == PipelineStage::Transcription
            && self.daemon.adopt_active_backend_for(&model.source).await
            && let Err(e) = self.daemon.persist_config().await
        {
            warn!("Failed to persist the active backend after a load: {e}");
        }
        Ok(LoadedModel {
            definition,
            instance,
        })
    }

    async fn idle_after(&self) -> Option<Duration> {
        self.daemon.config.read().await.idle_unload.after()
    }

    /// A recording runs every stage: the first while it captures, and the
    /// rest on its final transcript. Waiting for its models counts.
    fn in_use(&self) -> bool {
        self.daemon.recording.is_busy()
    }

    fn announce(&self, event: StageEvent<'_>) {
        let daemon = &self.daemon;
        let stage = self.stage.position();
        match event {
            StageEvent::Loading { model } => {
                daemon.broadcast_model_loading_status(&model.name, self.stage);
            }
            StageEvent::Loaded { model, device } => {
                daemon.broadcast_model_active(&model.name, &model.source, device, self.stage);
            }
            StageEvent::Unloaded => daemon.announce_stage_empty(self.stage),
            StageEvent::LoadFailed { .. } => {
                daemon.download_manager.clear_download(stage);
                daemon.announce_stage_empty(self.stage);
            }
            StageEvent::SwitchingDevice { model, from, to } => {
                daemon
                    .events
                    .publish_daemon_status(DaemonStatusEvent::SwitchingDevice {
                        from_device: from.to_string(),
                        target_device: to.to_string(),
                        model: model.name.clone(),
                        stage,
                    });
            }
            StageEvent::LoadingOnDevice { model, device } => {
                daemon
                    .events
                    .publish_daemon_status(DaemonStatusEvent::LoadingModelForDevice {
                        model: model.name.clone(),
                        target_device: device.to_string(),
                        stage,
                    });
            }
            StageEvent::DeviceSwitchFailed {
                model,
                device,
                error,
            } => {
                daemon
                    .events
                    .publish_daemon_status(DaemonStatusEvent::DeviceSwitchError {
                        error: error.to_string(),
                        failed_device: device.to_string(),
                        model: model.name.clone(),
                        stage,
                    });
            }
            StageEvent::DeviceSwitched {
                model,
                device,
                preferred,
            } => {
                daemon
                    .events
                    .publish_daemon_status(DaemonStatusEvent::Ready {
                        model_loaded: true,
                        model_name: Some(model.name.clone()),
                        actual_device: Some(device.to_string()),
                        preferred_device: Some(preferred.to_string()),
                        stage,
                    });
            }
        }
    }

    async fn select(&self, model: &ModelId) {
        let (name, source) = (model.name.clone(), model.source.clone());
        let persist = match self.stage {
            PipelineStage::Transcription => {
                self.daemon
                    .set_config_field(|c| c.update_preferred_model(name, source))
                    .await
            }
            PipelineStage::PostProcessor => {
                self.daemon
                    .set_config_field(|c| c.enable_post_processor(name, source))
                    .await
            }
        };
        if let Err(e) = persist {
            warn!("Failed to persist the selection of {model}: {e}");
        }
    }

    async fn switch_off(&self) -> bool {
        let was_on = {
            let mut config = self.daemon.config.write().await;
            match self.stage {
                PipelineStage::Transcription => {
                    let was_on = config.transcription.is_active();
                    config.disable_transcription();
                    was_on
                }
                PipelineStage::PostProcessor => {
                    let was_on = config.post_processor.is_active();
                    config.disable_post_processor();
                    was_on
                }
            }
        };
        // Persisted, so a restart stays off rather than load the model again.
        if was_on && let Err(e) = self.daemon.persist_config().await {
            warn!(
                "Failed to persist switching stage {} off: {e}",
                self.stage.position()
            );
        }
        was_on
    }

    async fn set_device(&self, model: &ModelId, device: &str) {
        let persist = self
            .daemon
            .set_config_field(|c| {
                c.update_model_device(&model.source, &model.name, Some(device.to_string()));
            })
            .await;
        if let Err(e) = persist {
            warn!("Failed to persist the device of {model}: {e}");
        }
    }

    fn shutdown_signal(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut shutdown = self.daemon.shutdown_tx.subscribe();
        async move {
            let _ = shutdown.recv().await;
        }
    }
}
