// SPDX-License-Identifier: GPL-3.0-only
//! Stage 2 of the pipeline — selecting the transcript post-processor.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline.md` (stage 2).
//!
//! Reached over `/stt/pipeline/2` and `/stt/pipeline/2/model`, the same paths every
//! stage answers: the first records *which backend* provides the post-processor
//! (cheap, cannot fail for runtime reasons), the second runs one of its models
//! (the step that downloads, loads and can fail). `POST` enables, `DELETE`
//! disables — there is no `enabled` field on the wire, only in the reported
//! state.
//!
//! The selection is stored as the same `(model, source)` identity pair every
//! other model selection uses, and is kept separately from the `enabled` flag
//! so disabling does not discard the user's choice.

use log::{info, warn};
use super_voice_shared::models::protocol::{DaemonResponse, ErrorCode};

use crate::daemon::device_management::PipelineStage;
use crate::daemon::types::SuperSTTDaemon;
use super_engine_daemon::stage::{Load, ModelId, StageExt, Unload};

impl SuperSTTDaemon {
    /// Run final transcripts through `model` — the post-processing twin of
    /// `POST /active_model`.
    ///
    /// An empty `source` resolves to the selected post-processor backend, so a
    /// client that has already chosen one names only the model.
    pub async fn handle_set_post_processor(&self, model: String, source: String) -> DaemonResponse {
        // Loading and unloading a backend instance during a recording is the
        // same hazard as switching the transcription model mid-recording.
        if let Some(resp) = self.guard_model_mutation("change the post-processor") {
            return resp;
        }

        // Resolve the backend the same way `set_model` does: an omitted source
        // means "the one already selected", and having none selected is the
        // caller's mistake, not an ambiguous lookup across every backend.
        let source = if source.is_empty() {
            let selected = self.config.read().await.post_processor.source.clone();
            if selected.is_empty() {
                return DaemonResponse::error_with_code(
                    ErrorCode::InvalidBackend,
                    "No post-processing backend is selected, so there is nothing to \
                     resolve the model against. Select one with POST /stt/pipeline/2, \
                     or name a source.",
                );
            }
            selected
        } else {
            source
        };

        // Validate before persisting: a selection that resolves to nothing (or
        // to a transcription model) would be stored, fail to load, and leave
        // the user with a setting that silently does nothing.
        let Some(definition) = self.resolve_definition(&model, &source).await else {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidModel,
                &format!(
                    "No installed backend serves the post-processing model {model} \
                     (source={source}). Install the backend or check the model name."
                ),
            );
        };
        if !definition.product.role.is_post_processor() {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidModel,
                &format!(
                    "Model {model} is a transcription model, not a post-processing \
                     model. Run it in the transcription stage with \
                     POST /stt/pipeline/1/model instead."
                ),
            );
        }
        // The choice is recorded before the load, so it is saved even when
        // the load fails: post-processing then passes text through, the
        // best-effort policy the pipeline follows. The failure is reported in
        // the message rather than failing the call.
        let switched = self
            .stage(PipelineStage::PostProcessor)
            .switch_to(ModelId::new(model.clone(), source.clone()))
            .await;
        let load_note = match &switched {
            Load::Loaded { .. } | Load::Unchanged => String::new(),
            Load::Failed { error, .. } => {
                warn!("Post-processor selected but not loaded: {error:#}");
                format!(" (not loaded: {error:#})")
            }
            Load::Superseded => " (not loaded: a newer model load replaced it)".to_string(),
            Load::Cancelled => " (not loaded: the daemon is shutting down)".to_string(),
        };

        self.publish_settings_changed("post_processor");
        info!("Post-processing enabled (model={model}, source={source})");

        let payload = self.post_processor_payload().await;
        DaemonResponse::success()
            .with_post_processor(payload)
            .with_message(format!("Post-processing enabled{load_note}"))
    }

    /// Stop running the post-processor, keeping the backend selected — the twin
    /// of `DELETE /active_model`, which unloads a model but leaves its backend
    /// in place. Use [`Self::handle_clear_post_processor_backend`] to forget the
    /// selection entirely.
    pub async fn handle_clear_post_processor(&self) -> DaemonResponse {
        if let Some(resp) = self.guard_model_mutation("change the post-processor") {
            return resp;
        }
        // Switched off as every stage is: after any load in flight, whose
        // model is the one unloaded, and persisted so a restart stays off.
        let message = match self.stage(PipelineStage::PostProcessor).unload().await {
            Unload::Unloaded { .. } | Unload::NothingToUnload => "Post-processing disabled",
            Unload::Superseded => "A newer model load replaces the post-processor",
            Unload::Cancelled => {
                return DaemonResponse::error(
                    "Disabling post-processing stopped: the daemon is shutting down",
                );
            }
        };
        self.publish_settings_changed("post_processor");
        info!("{message}");

        let payload = self.post_processor_payload().await;
        DaemonResponse::success()
            .with_post_processor(payload)
            .with_message(message.to_string())
    }

    /// Select the backend that provides the post-processor — the twin of
    /// `POST /active_backend`.
    ///
    /// Records *which* backend and validates that it serves one, without
    /// loading anything, so it cannot fail for runtime reasons.
    pub async fn handle_set_post_processor_backend(&self, source: String) -> DaemonResponse {
        if let Some(resp) = self.guard_model_mutation("change the post-processor") {
            return resp;
        }

        // A backend serving no post-processor would leave the user picking from
        // an empty list with nothing saying why.
        if !self.backend_serves_role(&source, true).await {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidBackend,
                &format!(
                    "Backend {source} serves no post-processing model. If it \
                     transcribes, select it for that stage with POST /stt/pipeline/1."
                ),
            );
        }

        // Switching backends drops whatever was running: the model belonged to
        // the old one. `select_post_processor_backend` clears it in config; the
        // unload here makes the runtime match.
        let switching = self.config.read().await.post_processor.source != source;
        if switching {
            self.unload_post_processor_announced().await;
        }
        let persist = self
            .set_config_field(|c| c.select_post_processor_backend(source.clone()))
            .await;

        self.publish_settings_changed("post_processor");
        info!("Post-processor backend: {source}");

        let payload = self.post_processor_payload().await;
        Self::settings_saved(
            DaemonResponse::success().with_post_processor(payload),
            format!("Post-processor backend: {source}"),
            persist,
        )
    }

    /// Deselect the post-processor backend: unload, and forget the model with
    /// it. The twin of `DELETE /active_backend`.
    pub async fn handle_clear_post_processor_backend(&self) -> DaemonResponse {
        if let Some(resp) = self.guard_model_mutation("change the post-processor") {
            return resp;
        }
        self.unload_post_processor_announced().await;
        let persist = self
            .set_config_field(crate::config::DaemonConfig::clear_post_processor_backend)
            .await;
        self.publish_settings_changed("post_processor");
        info!("Post-processor backend cleared");

        let payload = self.post_processor_payload().await;
        Self::settings_saved(
            DaemonResponse::success().with_post_processor(payload),
            "Post-processor backend cleared".to_string(),
            persist,
        )
    }

    /// The `{ enabled, model, source, loaded }` body both the getter and the
    /// setters answer with, so the shape cannot drift between them.
    async fn post_processor_payload(&self) -> serde_json::Value {
        let (enabled, model, source) = {
            let config = self.config.read().await;
            let pp = &config.post_processor;
            (pp.enabled, pp.model.clone(), pp.source.clone())
        };
        serde_json::json!({
            "enabled": enabled,
            // Null rather than "" for an empty selection: the field is an
            // identity, and an empty string is not one.
            "model": (!model.is_empty()).then_some(model),
            "source": (!source.is_empty()).then_some(source),
            "loaded": self.post_processor_loaded().await,
        })
    }
}
