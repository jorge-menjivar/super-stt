// SPDX-License-Identifier: GPL-3.0-only
//! `GET /pipeline` — the ordered stages a transcript passes through.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline.md`.
//!
//! Stage 1 turns audio into text; every later stage rewrites the text the one
//! before it produced. Today there are exactly two — transcription and
//! post-processing — but they are addressed by position precisely so a third
//! can be appended without inventing a third endpoint for it.
//!
//! A stage and its model are reported separately, and the split is what makes
//! the positions interchangeable. A stage is a *backend selection*: durable,
//! surviving unloads and restarts. Its model slot is a selection too, but one
//! that also has a runtime — loaded or not, on some device, possibly still
//! downloading. Stage 1 used to conflate the two, reporting the model it had
//! loaded rather than the model it had chosen, so `loaded` told a client
//! nothing it could not read off `model` and an unload emptied the card.
//!
//! This module only *reports*. Each stage is filled through the same handlers
//! its own endpoint always used, so there is one implementation of "select a
//! backend" per stage, not two.

use super_stt_registry_types::manifest::Device;
use super_stt_shared::models::protocol::{
    DaemonResponse, POST_PROCESSOR_STAGE, StageModelDevice, StageModelReport, StageReport,
    StageRole, StageSwitch, SwitchDownload, SwitchTarget, TRANSCRIPTION_STAGE,
};

use crate::daemon::device_management::PipelineStage;
use crate::daemon::types::{SuperSTTDaemon, normalize_device};
use crate::stt_models::backends;

impl SuperSTTDaemon {
    /// Report every stage in order.
    pub async fn handle_get_pipeline(&self) -> DaemonResponse {
        let stages = vec![
            self.transcription_stage().await,
            self.post_processor_stage().await,
        ];
        DaemonResponse::success()
            .with_pipeline(stages)
            .with_message("Pipeline retrieved successfully".to_string())
    }

    /// `GET /pipeline/{stage}/model` — one stage's model slot.
    ///
    /// Every stage answers this the same way, which is the whole point of it:
    /// `model` is what the stage is pointed at, `loaded` whether that is
    /// running, and `device` which accelerator it runs on.
    pub async fn handle_get_stage_model(&self, stage: PipelineStage) -> DaemonResponse {
        let report = self.stage_model(stage).await;
        DaemonResponse::success()
            .with_message(match report.model.as_deref() {
                Some(model) if report.loaded => {
                    format!("Stage {} is running {model}", stage.position())
                }
                Some(model) => format!(
                    "Stage {} has {model} selected, not loaded",
                    stage.position()
                ),
                None => format!("Stage {} has no model selected", stage.position()),
            })
            .with_stage_model(report)
    }

    /// Stage 1: the backend that turns audio into text.
    ///
    /// Read from the same state `/active_backend` reports, so the two views
    /// cannot disagree. The model it runs is [`Self::stage_model`].
    async fn transcription_stage(&self) -> StageReport {
        // A backend can be selected with nothing loaded, and stays selected
        // through an unload — this is the selection, not the runtime.
        let source = self.active_backend_source().await;
        StageReport {
            stage: TRANSCRIPTION_STAGE,
            role: StageRole::Transcription,
            name: self.backend_name(source.as_deref()).await,
            source,
            enabled: self.config.read().await.transcription.is_active(),
        }
    }

    /// Stage 2: the backend whose post-processor rewrites each final transcript.
    async fn post_processor_stage(&self) -> StageReport {
        let (enabled, source) = {
            let config = self.config.read().await;
            (
                config.post_processor.is_active(),
                config.post_processor.source.clone(),
            )
        };
        let source = (!source.is_empty()).then_some(source);
        StageReport {
            stage: POST_PROCESSOR_STAGE,
            role: StageRole::PostProcessor,
            name: self.backend_name(source.as_deref()).await,
            source,
            enabled,
        }
    }

    /// One stage's model slot: the selection, whether it is up, the device it
    /// runs on, and the load still in flight.
    ///
    /// Both stages are built here rather than once per stage: the selection
    /// lives in a different config field for each, and everything after that —
    /// resolving the device, deciding whether the selection is the instance
    /// actually loaded — is identical, and was the part that used to differ.
    async fn stage_model(&self, stage: PipelineStage) -> StageModelReport {
        let (model, source) = {
            let config = self.config.read().await;
            match stage {
                PipelineStage::Transcription => (
                    config.transcription.preferred_model.clone(),
                    config.transcription.preferred_source.clone(),
                ),
                PipelineStage::PostProcessor => (
                    config.post_processor.model.clone(),
                    config.post_processor.source.clone(),
                ),
            }
        };
        let selection = (!model.is_empty() && !source.is_empty()).then_some((model, source));
        let (model, device, loaded) = match &selection {
            Some((model, source)) => {
                // Whether *this* selection is the instance that is up, not
                // merely whether the stage has something loaded: mid-switch the
                // two differ, and a client reading `loaded` about the model it
                // was just told is selected has to get an answer about that
                // model.
                let running = self.running_stage_device(stage, source, model).await;
                (
                    Some(model.clone()),
                    Some(
                        self.stage_model_device(source, model, running.clone())
                            .await,
                    ),
                    running.is_some(),
                )
            }
            None => (None, None, false),
        };

        StageModelReport {
            stage: stage.position(),
            model,
            loaded,
            device,
            switch: self.stage_switch(stage),
        }
    }

    /// The accelerator `(source, model)` is running on right now, or `None`
    /// when it is not the instance loaded in `stage`.
    ///
    /// Read from the instance rather than from any preference, so a `gpu`
    /// choice that fell back to the CPU reports `cpu`. Doubles as the "is this
    /// selection loaded?" test, which is why the two cannot drift apart.
    async fn running_stage_device(
        &self,
        stage: PipelineStage,
        source: &str,
        model: &str,
    ) -> Option<String> {
        let slot = match stage {
            PipelineStage::Transcription => &self.model,
            PipelineStage::PostProcessor => &self.post_processor,
        };
        let guard = slot.read().await;
        let loaded = guard.as_ref()?;
        (loaded.definition.name == model && loaded.definition.source == source)
            .then(|| normalize_device(&loaded.instance.device()))
    }

    /// The device block for a stage's selection.
    ///
    /// Deliberately the shape `GET /pipeline/{stage}/model/{model}/device`
    /// answers with, minus `available_devices`: that list costs a fresh host
    /// probe, and a card fills its picker from the device endpoint once rather
    /// than on every poll of the stage's model.
    async fn stage_model_device(
        &self,
        source: &str,
        model: &str,
        running: Option<String>,
    ) -> StageModelDevice {
        let online = {
            let backends = self.backends.read().await;
            backends::find_model(&backends, model, source).is_some_and(|(_, def)| def.is_online())
        };
        if online {
            // The manifest's own sentinel: remote compute has no local device,
            // and nothing resolves locally whether it is loaded or not.
            return StageModelDevice {
                preference: Device::None.to_string(),
                resolved_accel: None,
            };
        }
        let preference = self.config.read().await.effective_device(source, model);
        let resolved_accel = match running {
            Some(actual) => Some(actual),
            // Not loaded: `cpu` needs no resolution, `gpu` has none yet — a
            // client is never told a device resolved before a load confirmed it.
            None => (preference == "cpu").then(|| preference.clone()),
        };
        StageModelDevice {
            preference,
            resolved_accel,
        }
    }

    /// In-flight load/download progress for `stage`, or `null` when that stage
    /// is not fetching anything.
    ///
    /// The daemon runs one model operation at a time, but not always for the
    /// same stage: a post-processor's download would otherwise surface as
    /// stage 1's, which is exactly what the `stage` the tracker now reports is
    /// for.
    fn stage_switch(&self, stage: PipelineStage) -> Option<StageSwitch> {
        let p = self.handle_get_download_status(stage).download_progress?;
        Some(StageSwitch {
            phase: p.status,
            target: SwitchTarget {
                model: p.model_name,
                source: p.source,
            },
            started_at: p.started_at,
            download: SwitchDownload {
                current_file: p.current_file,
                file_index: p.file_index,
                total_files: p.total_files,
                bytes_downloaded: p.bytes_downloaded,
                total_bytes: p.total_bytes,
                percentage: p.percentage,
                eta_seconds: p.eta_seconds,
            },
        })
    }

    /// Whether an installed backend serves at least one model a given stage
    /// can run.
    ///
    /// Every stage needs this: filling a stage with a backend that serves
    /// nothing for it leaves the user staring at an empty model picker with no
    /// reason given. Before post-processors existed every backend transcribed,
    /// so stage 1 never had to ask — it does now.
    pub(crate) async fn backend_serves_role(&self, source: &str, post_processor: bool) -> bool {
        let backends = self.backends.read().await;
        backends.iter().filter(|b| b.source == source).any(|b| {
            b.models
                .iter()
                .any(|m| m.is_post_processor() == post_processor)
        })
    }

    /// A backend's display name, for a stage to report beside its `source`.
    async fn backend_name(&self, source: Option<&str>) -> Option<String> {
        let source = source?;
        let backends = self.backends.read().await;
        backends
            .iter()
            .find(|b| b.source == source)
            .map(|b| b.name.clone())
    }
}
