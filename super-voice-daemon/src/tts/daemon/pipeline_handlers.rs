// SPDX-License-Identifier: GPL-3.0-only
//! `GET /pipeline` — the ordered stages an utterance passes through.
//!
//! Super TTS has exactly one: stage 1 turns text into audio. It is still
//! addressed by position, and reported as a one-element list rather than a bare
//! object, so a stage put ahead of it — a text normalizer, an LLM rewriter —
//! becomes a second row in a list clients already walk instead of a second
//! endpoint family they have to learn about before they can use it.
//!
//! A stage and its model are reported separately, and that split is what makes
//! the positions interchangeable. A stage is a *backend selection*: durable,
//! surviving an unload and a restart, and unable to fail for runtime reasons.
//! Its model slot is a selection too, but one that also has a runtime — loaded
//! or not, on some device, possibly still downloading. Folding them into one
//! report is what lets a client read "a backend is chosen" as "speech will
//! work" and paint a ready UI over a daemon whose next `speak` will fail.
//!
//! This module only *reports*. The stage is filled through the same
//! `set_active_backend` / `set_model` handlers it always was, so there is one
//! implementation of "select a backend for synthesis", not two that can come to
//! disagree about what counts as selected.

use crate::tts::daemon::types::SuperTTSDaemon;
use super_voice_shared::tts::models::protocol::{
    DaemonResponse, SYNTHESIS_STAGE, StageModelReport, StageReport, StageRole, StageSwitch,
    SwitchDownload, SwitchTarget,
};

impl SuperTTSDaemon {
    /// Report every stage in order.
    pub async fn handle_get_pipeline(&self) -> DaemonResponse {
        let stages = vec![self.synthesis_stage().await];
        DaemonResponse {
            pipeline: Some(stages),
            ..DaemonResponse::success().with_message("Pipeline retrieved successfully".to_string())
        }
    }

    /// Stage 1: the backend that turns text into audio.
    ///
    /// Read through [`active_backend_source`](Self::active_backend_source), the
    /// same lookup `/active_backend` answers from, so the two views cannot
    /// disagree about which backend is selected — including on the case that
    /// lookup exists for, a backend uninstalled out from under the selection,
    /// where the stage correctly reports itself empty rather than naming a
    /// directory that is gone. The model it runs is
    /// [`stage_model`](Self::stage_model).
    async fn synthesis_stage(&self) -> StageReport {
        let source = self.active_backend_source().await;
        StageReport {
            stage: SYNTHESIS_STAGE,
            role: StageRole::Synthesis,
            name: self.backend_name(source.as_deref()).await,
            // Super TTS persists no separate on/off flag for the stage, so this
            // is derived from the one thing it does record: whether a backend
            // is selected. `POST /active_backend` (and a model switch, which
            // selects the model's backend) turns the stage on;
            // `DELETE /active_backend` turns it off and idles the daemon. That
            // is the whole of the user's choice today.
            //
            // Derived rather than invented on purpose. A field backed by state
            // nothing writes would answer the same value forever, and a client
            // would render a switch that never moves — worse than no switch at
            // all, because it looks like it works. When super-tts grows a real
            // stage toggle this reads it instead, and the wire shape does not
            // change.
            //
            // Note this stays `true` across an unload: unloading drops the
            // model and keeps the backend, so the stage is still switched on
            // with nothing running. That state is `loaded: false` on the model
            // slot, which is exactly the distinction the two reports keep apart.
            enabled: source.is_some(),
            source,
        }
    }

    /// Stage 1's model slot: the selection, whether it is up, the device it
    /// runs on, and the load still in flight.
    ///
    /// `model` is the *selection* and `loaded` says whether that selection is
    /// the instance actually running. Asking about the selection rather than
    /// about the loaded model is what makes the answer meaningful mid-switch,
    /// when the daemon is loading one model while another (or none) is up: a
    /// client that was just told which model the stage points at has to get a
    /// `loaded` about *that* model, not about whatever happens to occupy the
    /// slot.
    pub(in crate::tts::daemon) async fn stage_model(&self) -> StageModelReport {
        let Some((model, source)) = self.stage_selection().await else {
            return StageModelReport {
                stage: SYNTHESIS_STAGE,
                model: None,
                loaded: false,
                device: None,
                // Still reported: the very first load of a fresh install is in
                // flight before anything has been selected, and a client with
                // no download to show would leave the user staring at an idle
                // card through a multi-gigabyte fetch.
                switch: self.stage_switch().await,
            };
        };
        let running = self.running_device(&source, &model).await;
        let device = self
            .model_device_view(&source, &model, running.clone())
            .await;
        StageModelReport {
            stage: SYNTHESIS_STAGE,
            model: Some(model),
            loaded: running.is_some(),
            device: Some(device),
            switch: self.stage_switch().await,
        }
    }

    /// The `(model, source)` stage 1 is pointed at, or `None` when nothing is
    /// selected.
    ///
    /// Read from the persisted preference, not from the loaded instance: the
    /// preference is what the daemon reloads at startup and the only record
    /// that still exists while a model is being swapped out. Today an unload
    /// clears it along with the instance, so the two collapse — but the
    /// distinction is what `loaded` is for, and reading the config here is what
    /// makes an unload that keeps its selection a one-line change rather than a
    /// protocol one.
    ///
    /// An empty `preferred_source` resolves against the active backend, exactly
    /// as [`selected_model`](Self::selected_model) resolves it and for
    /// the same reason: a config written before that key existed names a
    /// model and no backend, and reporting "nothing selected" for a config the
    /// daemon *would* load at its next start is a lie the client has no way to
    /// detect.
    async fn stage_selection(&self) -> Option<(String, String)> {
        let (model, source) = {
            let config = self.config.read().await;
            (
                config.synthesis.preferred_model.clone(),
                config.synthesis.preferred_source.clone(),
            )
        };
        if model.is_empty() {
            return None;
        }
        let source = if source.is_empty() {
            self.active_backend_source().await?
        } else {
            source
        };
        Some((model, source))
    }

    /// The load or download in flight for stage 1, or `None` when nothing is
    /// being fetched.
    ///
    /// Mapped from the daemon's existing download tracker rather than tracked
    /// twice: `phase` is that tracker's `status` (`verifying`, `downloading`,
    /// `loading_model`, `cancelled`, `completed`, `error`), under the name the
    /// pipeline shape gives it.
    async fn stage_switch(&self) -> Option<StageSwitch> {
        let progress = self.handle_get_download_status().download_progress?;
        // The tracker records the model being fetched but not the backend
        // serving it — there is one download at a time and, until this report,
        // one reader that already knew which backend it had asked for. The
        // switch path records the target's backend as the active one *before*
        // the load begins (see `handle_set_model`), so the active backend is
        // that backend. Reading it off the *loaded* model instead would name
        // the previous backend for the whole of a cross-backend switch, and
        // hardcoding an empty string would make the target unidentifiable
        // whenever two installed backends serve the same model name — which is
        // the case `(model, source)` pairs exist to disambiguate.
        let source = self.active_backend_source().await.unwrap_or_default();
        Some(StageSwitch {
            phase: progress.status,
            target: SwitchTarget {
                model: progress.model_name,
                source,
            },
            started_at: progress.started_at,
            download: SwitchDownload {
                current_file: progress.current_file,
                file_index: progress.file_index,
                total_files: progress.total_files,
                bytes_downloaded: progress.bytes_downloaded,
                total_bytes: progress.total_bytes,
                percentage: progress.percentage,
                eta_seconds: progress.eta_seconds,
            },
            load: progress.load,
        })
    }

    /// A backend's display name, for the stage to report beside its `source`.
    ///
    /// Looked up in the same discovered-backend list the source came from, so
    /// a backend that vanished between the two reads reports `null` here rather
    /// than a stale name — a stage naming a backend it cannot find is worse
    /// than a stage naming none, because a client would offer its models.
    async fn backend_name(&self, source: Option<&str>) -> Option<String> {
        let source = source?;
        let backends = self.backends.read().await;
        backends
            .iter()
            .find(|b| b.source == source)
            .map(|b| b.name.clone())
    }
}
