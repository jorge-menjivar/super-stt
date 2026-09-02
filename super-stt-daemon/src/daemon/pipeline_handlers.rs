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
//! This module only *reports*. Each stage is filled through the same handlers
//! its own endpoint always used, so there is one implementation of "select a
//! backend" per stage, not two.

use super_stt_shared::models::protocol::DaemonResponse;

use crate::daemon::types::SuperSTTDaemon;

/// Wire name for a stage's role, matching `ModelRole` in the manifest.
const ROLE_TRANSCRIPTION: &str = "transcription";
const ROLE_POST_PROCESSOR: &str = "post_processor";

impl SuperSTTDaemon {
    /// Report every stage in order.
    pub async fn handle_get_pipeline(&self) -> DaemonResponse {
        let stages = serde_json::Value::Array(vec![
            self.transcription_stage().await,
            self.post_processor_stage().await,
        ]);
        DaemonResponse::success()
            .with_pipeline(stages)
            .with_message("Pipeline retrieved successfully".to_string())
    }

    /// Stage 1: the backend and model that turn audio into text.
    ///
    /// Read from the same state `/active_backend` and `/active_model` report,
    /// so the two views cannot disagree.
    async fn transcription_stage(&self) -> serde_json::Value {
        let loaded = self.model.read().await;
        let definition = loaded.as_ref().map(|m| &m.definition);
        let (source, model) = match definition {
            Some(d) => (Some(d.source.clone()), Some(d.name.clone())),
            None => (None, None),
        };

        // A backend can be selected with nothing loaded, which is the state
        // `/active_backend` holds on its own; prefer the loaded model's source
        // and fall back to the selection.
        let source = match source {
            Some(s) => Some(s),
            None => self.active_backend_source().await,
        };
        let name = self.backend_name(source.as_deref()).await;

        // The device the model is actually on, not the user's cpu/gpu
        // preference — a stage reports where its work runs. Read directly
        // rather than through `handle_get_device`, which probes the host.
        let device = self.actual_device.read().await.clone();

        serde_json::json!({
            "stage": 1,
            "role": ROLE_TRANSCRIPTION,
            "source": source,
            "name": name,
            "model": model,
            "loaded": loaded.is_some(),
            "device": device,
            "switch": self.stage_switch(),
        })
    }

    /// In-flight load/download progress for a stage, or `null` when nothing is
    /// being fetched. Only stage 1 downloads today; the field is on every stage
    /// because any stage that loads weights can.
    fn stage_switch(&self) -> serde_json::Value {
        let progress = self.handle_get_download_status().download_progress;
        progress.map_or(serde_json::Value::Null, |p| {
            serde_json::json!({
                "phase":      p.status,
                "target":     { "model": p.model_name },
                "started_at": p.started_at,
                "download": {
                    "current_file":     p.current_file,
                    "file_index":       p.file_index,
                    "total_files":      p.total_files,
                    "bytes_downloaded": p.bytes_downloaded,
                    "total_bytes":      p.total_bytes,
                    "percentage":       p.percentage,
                    "eta_seconds":      p.eta_seconds,
                },
            })
        })
    }

    /// Stage 2: the post-processor that rewrites each final transcript.
    ///
    /// Also what `POST`/`DELETE /pipeline/2[/model]` answer with, so the
    /// shape cannot drift between a read and a write.
    pub(in crate::daemon) async fn post_processor_stage(&self) -> serde_json::Value {
        let (enabled, model, source, preferred_device) = {
            let config = self.config.read().await;
            let pp = &config.post_processor;
            (
                pp.enabled,
                pp.model.clone(),
                pp.source.clone(),
                pp.device.clone(),
            )
        };
        let source = (!source.is_empty()).then_some(source);
        let name = self.backend_name(source.as_deref()).await;

        serde_json::json!({
            "stage": 2,
            "role": ROLE_POST_PROCESSOR,
            "source": source,
            "name": name,
            "model": (!model.is_empty()).then_some(model),
            "loaded": self.post_processor_loaded().await,
            // Where the work runs, as stage 1 reports it: the accelerator the
            // loaded instance is actually on, null until one has loaded.
            "device": self.post_processor_device().await,
            // The stage's own `cpu`/`gpu` ask, which `device` is the answer
            // to. Null when it has none and follows stage 1's.
            "preferred_device": (!preferred_device.is_empty()).then_some(preferred_device),
            // Processor stages carry the user's on/off choice separately from
            // whether the model actually came up: a selection can be enabled
            // while its load failed, and transcripts then pass through.
            "enabled": enabled,
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
