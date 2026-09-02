// SPDX-License-Identifier: GPL-3.0-only
//! Stage 2 of the pipeline — selecting the transcript post-processor.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline.md` (stage 2).
//!
//! Reached over `/pipeline/2` and `/pipeline/2/model`, the same paths every
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
use super_stt_shared::models::protocol::{DaemonResponse, ErrorCode};

use crate::daemon::types::SuperSTTDaemon;

impl SuperSTTDaemon {
    /// Report the current selection: `{ enabled, model, source, loaded }`.
    ///
    /// `loaded` is the runtime fact, distinct from `enabled`: a selection can
    /// be enabled while its model failed to load, in which case transcripts
    /// pass through unprocessed.
    pub async fn handle_get_post_processor(&self) -> DaemonResponse {
        let payload = self.post_processor_payload().await;
        DaemonResponse::success()
            .with_post_processor(payload)
            .with_message("Post-processor setting retrieved successfully".to_string())
    }

    /// Run final transcripts through `model` — the post-processing twin of
    /// `POST /active_model`.
    ///
    /// An empty `source` resolves to the selected post-processor backend, so a
    /// client that has already chosen one names only the model. `device` is
    /// the stage's own `cpu`/`gpu` preference; `None` keeps the stored one.
    pub async fn handle_set_post_processor(
        &self,
        model: String,
        source: String,
        device: Option<String>,
    ) -> DaemonResponse {
        // Loading and unloading a backend instance during a recording is the
        // same hazard as switching the transcription model mid-recording.
        if let Some(resp) = self.guard_model_mutation("change the post-processor").await {
            return resp;
        }

        // Validate and normalize (`cuda`/`metal` → `gpu`) before anything is
        // persisted, with the same code and wording `/active_device` uses for
        // the same mistake.
        let device = match device {
            None => None,
            Some(raw) => match crate::daemon::device_management::parse_device_preference(&raw) {
                Some(normalized) => Some(normalized),
                None => {
                    return DaemonResponse::error_with_code(
                        ErrorCode::InvalidDevice,
                        &format!("Invalid device '{raw}'. Must be 'cpu' or 'gpu'"),
                    );
                }
            },
        };

        // Resolve the backend the same way `set_model` does: an omitted source
        // means "the one already selected", and having none selected is the
        // caller's mistake, not an ambiguous lookup across every backend.
        let source = if source.is_empty() {
            let selected = self.config.read().await.post_processor.source.clone();
            if selected.is_empty() {
                return DaemonResponse::error_with_code(
                    ErrorCode::InvalidBackend,
                    "No post-processing backend is selected, so there is nothing to \
                     resolve the model against. Select one with POST /pipeline/2, \
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
        if !definition.is_post_processor() {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidModel,
                &format!(
                    "Model {model} is a transcription model, not a post-processing \
                     model. Run it in the transcription stage with \
                     POST /pipeline/1/model instead."
                ),
            );
        }
        if definition.is_online() && !self.config.read().await.online.allow_online_models {
            return DaemonResponse::error_with_code(
                ErrorCode::OnlineModelsDisabled,
                "Online models are disabled. Enable 'Allow Online Models' in settings first.",
            );
        }

        let persist = self
            .set_config_field(|c| {
                c.enable_post_processor(model.clone(), source.clone(), device.clone());
            })
            .await;

        // A load failure is reported in the message but does not fail the call:
        // the setting is saved, and post-processing degrades to passing text
        // through — the same best-effort policy the pipeline follows.
        let mut load_note = String::new();
        if let Err(e) = self.load_configured_post_processor().await {
            warn!("Post-processor selected but not loaded: {e}");
            load_note = format!(" (not loaded: {e})");
        }

        self.publish_settings_changed("post_processor");
        info!("Post-processing enabled (model={model}, source={source})");

        let payload = self.post_processor_payload().await;
        Self::settings_saved(
            DaemonResponse::success().with_post_processor(payload),
            format!("Post-processing enabled{load_note}"),
            persist,
        )
    }

    /// Stop running the post-processor, keeping the backend selected — the twin
    /// of `DELETE /active_model`, which unloads a model but leaves its backend
    /// in place. Use [`Self::handle_clear_post_processor_backend`] to forget the
    /// selection entirely.
    pub async fn handle_clear_post_processor(&self) -> DaemonResponse {
        if let Some(resp) = self.guard_model_mutation("change the post-processor").await {
            return resp;
        }
        self.unload_post_processor().await;
        let persist = self
            .set_config_field(crate::config::DaemonConfig::disable_post_processor)
            .await;
        self.publish_settings_changed("post_processor");
        info!("Post-processing disabled");

        let payload = self.post_processor_payload().await;
        Self::settings_saved(
            DaemonResponse::success().with_post_processor(payload),
            "Post-processing disabled".to_string(),
            persist,
        )
    }

    /// Select the backend that provides the post-processor — the twin of
    /// `POST /active_backend`.
    ///
    /// Records *which* backend and validates that it serves one, without
    /// loading anything, so it cannot fail for runtime reasons.
    pub async fn handle_set_post_processor_backend(&self, source: String) -> DaemonResponse {
        if let Some(resp) = self.guard_model_mutation("change the post-processor").await {
            return resp;
        }

        // A backend serving no post-processor would leave the user picking from
        // an empty list with nothing saying why.
        if !self.backend_serves_role(&source, true).await {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidBackend,
                &format!(
                    "Backend {source} serves no post-processing model. If it \
                     transcribes, select it for that stage with POST /pipeline/1."
                ),
            );
        }

        // Switching backends drops whatever was running: the model belonged to
        // the old one. `select_post_processor_backend` clears it in config; the
        // unload here makes the runtime match.
        let switching = self.config.read().await.post_processor.source != source;
        if switching {
            self.unload_post_processor().await;
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
        if let Some(resp) = self.guard_model_mutation("change the post-processor").await {
            return resp;
        }
        self.unload_post_processor().await;
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

    /// The stage object every setter answers with — the same one
    /// `GET /pipeline/2` reads, so a write's echo and the next read cannot
    /// disagree.
    async fn post_processor_payload(&self) -> serde_json::Value {
        self.post_processor_stage().await
    }
}
