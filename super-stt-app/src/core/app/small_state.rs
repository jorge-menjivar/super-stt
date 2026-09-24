// SPDX-License-Identifier: GPL-3.0-only

use crate::ui::messages::Message;
use cosmic::prelude::*;

use super::{AppModel, DeviceState, ModelOperationState};
use crate::state::device_offers::STT_STAGE;

/// Stall threshold for the model-switch watchdog. If a switch makes no
/// progress for this long, the UI flips to an error instead of spinning
/// forever. Generous so the untracked `loading_model` phase (no progress
/// events while weights load onto the device) or a genuinely slow download
/// isn't cut off prematurely — and the still-open `set_model` POST
/// self-corrects to `Ready`/`Error` if the switch later finishes anyway.
const SWITCH_STALL_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(5);

impl AppModel {
    /// Whether `stage` is free to start a model operation. Asked per stage:
    /// the daemon runs the stages independently, so a post-processor download
    /// must leave the transcription card's Load button live.
    pub fn is_model_ready(&self, stage: u32) -> bool {
        self.model_operations.is_ready(stage)
    }

    /// Clear the locally-tracked loaded model — its name and source.
    /// Called wherever the daemon goes idle (unload, failed switch, download
    /// error/cancel) or the selection is optimistically dropped. Adjacent state
    /// (operation status, staged pickers, active backend) is each caller's
    /// responsibility.
    pub(in crate::core::app) fn clear_loaded_model(&mut self) {
        self.current_model.clear();
        self.current_source.clear();
    }

    /// Set model to the provisioning state — its files are being verified or
    /// downloaded, and which of the two is in `progress.status`. `stage` is the
    /// pipeline position being provisioned for, straight off the daemon's
    /// report.
    pub(in crate::core::app) fn set_model_provisioning(
        &mut self,
        target_model: String,
        progress: super_stt_shared::models::protocol::DownloadProgress,
        stage: u32,
    ) {
        // A tick is progress, so it restarts that stage's stall clock.
        self.model_operations.start(
            stage,
            ModelOperationState::Provisioning {
                target_model,
                progress: Box::new(progress),
            },
        );
    }

    /// Set model to loading state, for the stage running the model.
    pub(in crate::core::app) fn set_model_loading(
        &mut self,
        target_model: String,
        status_message: String,
        stage: u32,
    ) {
        self.set_model_loading_with(target_model, status_message, stage, None);
    }

    /// [`Self::set_model_loading`], with what the backend reports of its load.
    fn set_model_loading_with(
        &mut self,
        target_model: String,
        status_message: String,
        stage: u32,
        load: Option<super_stt_shared::models::protocol::LoadProgress>,
    ) {
        // Entering a switch starts that stage's stall clock (see PingTimeout).
        self.model_operations.start(
            stage,
            ModelOperationState::Loading {
                target_model,
                status_message,
                load,
            },
        );
    }

    /// Set device to switching state
    pub(in crate::core::app) fn set_device_switching(
        &mut self,
        target_device: String,
        status_message: String,
    ) {
        self.device_state = DeviceState::Switching {
            target_device,
            status_message,
        };
    }

    /// Apply a download-progress snapshot to the model operation state.
    ///
    /// Both the polling path (`DownloadProgressUpdate`) and the daemon-event
    /// path (`download_progress` event) use exactly the same mapping:
    /// - `"loading_model"` → `Loading` state
    /// - `"error"` → `Error` state, surfacing the daemon's failure detail
    /// - `"completed" | "cancelled"` → no state change (`ModelChanged` /
    ///   `DownloadCancelled` carry those transitions)
    /// - `"verifying" | "downloading"` (and anything unrecognised) →
    ///   `Provisioning` state, which keeps the snapshot verbatim so the card
    ///   can word itself from `progress.status`. The two phases share a state
    ///   deliberately: both are byte-tracked work on the model's files, and
    ///   every gate that asks "is an operation in flight?" wants the same
    ///   answer for both.
    ///
    /// Any progress event also resets the stall watchdog (see `PingTimeout`).
    pub(in crate::core::app) fn apply_download_progress(
        &mut self,
        progress: &super_stt_shared::models::protocol::DownloadProgress,
    ) {
        let target_model = progress.model_name.clone();
        // The daemon says which stage it is provisioning for, so the progress
        // lands on the card that started it rather than on whichever card
        // happens to render the operation.
        let stage = progress.stage;
        match progress.status.as_str() {
            "loading_model" => {
                self.set_model_loading_with(
                    target_model,
                    "Loading model into memory...".to_string(),
                    stage,
                    progress.load.clone(),
                );
            }
            "error" => {
                // The daemon broadcasts a terminal `error` for any switch
                // failure (download, spawn, or load) carrying the failure
                // detail. Surface it as the model-switch error banner — this is
                // the authoritative event-driven path; the now-untimed
                // `set_model` POST's `ModelError` lands consistently after.
                let message = progress
                    .error
                    .clone()
                    .unwrap_or_else(|| "Model switch failed".to_string());
                self.model_operations
                    .set(stage, ModelOperationState::Error { message });
                // A failed switch leaves that stage idle — clear the selection
                // so the UI doesn't show a model that isn't loaded (mirrors the
                // `ModelError` handler). Only stage 1's identity is kept here,
                // so a post-processor's failure must not clear it.
                if stage == STT_STAGE {
                    self.clear_loaded_model();
                }
            }
            "completed" | "cancelled" => {
                // State will be updated by subsequent daemon events
                log::info!("Download finished with status: {}", progress.status);
            }
            _ => {
                // "verifying", "downloading", and anything the daemon adds
                // later: files are being provisioned.
                self.set_model_provisioning(target_model, progress.clone(), stage);
            }
        }
    }

    /// Model-switch stall watchdog, called on each `PingTimeout` tick. While a
    /// switch is in flight, a progress event (download tick, `loading_model`,
    /// or the initial `set_model_loading`) resets that stage's clock; if none
    /// arrives within [`SWITCH_STALL_TIMEOUT`], flip that stage to an error so
    /// the UI doesn't spin forever. Each stage is timed on its own, so a busy
    /// one cannot keep a stalled one alive. No-op outside a switch.
    pub(in crate::core::app) fn check_switch_stall(&mut self) {
        let stalled = self.model_operations.fail_stalled(
            SWITCH_STALL_TIMEOUT,
            "Model switch stalled — the daemon stopped reporting progress.",
        );
        for stage in stalled {
            log::warn!(
                "Model switch on stage {stage} stalled: no progress for {}s",
                SWITCH_STALL_TIMEOUT.as_secs()
            );
        }
    }

    /// Updates the header and window titles.
    pub(in crate::core::app) fn update_title(&mut self) -> Task<cosmic::Action<Message>> {
        let mut window_title = "Super STT".to_string();

        if let Some(page) = self.nav.text(self.nav.active()) {
            window_title.push_str(" — ");
            window_title.push_str(page);
        }

        if let Some(id) = self.core.main_window_id() {
            self.set_window_title(window_title, id)
        } else {
            Task::none()
        }
    }
}
