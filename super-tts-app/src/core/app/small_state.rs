// SPDX-License-Identifier: GPL-3.0-only

use crate::ui::messages::Message;
use cosmic::prelude::*;

use super::{AppModel, DeviceState, ModelOperationState};

/// Stall threshold for the model-switch watchdog. If a switch makes no
/// progress for this long, the UI flips to an error instead of spinning
/// forever. Generous so the untracked `loading_model` phase (no progress
/// events while weights load onto the device) or a genuinely slow download
/// isn't cut off prematurely — and the still-open `set_model` POST
/// self-corrects to `Ready`/`Error` if the switch later finishes anyway.
const SWITCH_STALL_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(5);

impl AppModel {
    /// Check if model is ready
    pub fn is_model_ready(&self) -> bool {
        matches!(self.model_operation_state, ModelOperationState::Ready)
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

    /// Drop the staged pick together with the per-model device answer it was
    /// drawn from.
    ///
    /// The two go together because the answer is only meaningful for the model
    /// it describes. Clearing the pick alone leaves a device list and a stored
    /// preference behind, and the next model staged from a different backend
    /// would be offered them for as long as its own answer took to arrive —
    /// with nothing on screen to say the picker was showing another model's
    /// devices.
    pub(in crate::core::app) fn clear_staged_model(&mut self) {
        self.models_page.staged_model = None;
        self.models_page.staged_device = None;
        self.model_device = None;
        self.model_device_for = None;
    }

    /// Set model to the provisioning state — its files are being verified or
    /// downloaded. Which of the two is in `progress.status`.
    pub(in crate::core::app) fn set_model_provisioning(
        &mut self,
        target_model: String,
        progress: super_tts_shared::models::protocol::DownloadProgress,
    ) {
        self.model_operation_state = ModelOperationState::Provisioning {
            target_model,
            progress,
        };
    }

    /// Set model to loading state
    pub(in crate::core::app) fn set_model_loading(
        &mut self,
        target_model: String,
        status_message: String,
    ) {
        self.set_model_loading_with(target_model, status_message, None);
    }

    /// [`Self::set_model_loading`], with what the backend reports of its load.
    fn set_model_loading_with(
        &mut self,
        target_model: String,
        status_message: String,
        load: Option<super_tts_shared::models::protocol::LoadProgress>,
    ) {
        // Entering a switch starts the stall watchdog clock (see PingTimeout).
        self.last_switch_progress_at = Some(std::time::Instant::now());
        self.model_operation_state = ModelOperationState::Loading {
            target_model,
            status_message,
            load,
        };
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
    ///   every gate that asks "is a switch in flight?" wants the same answer
    ///   for both.
    ///
    /// Any progress event also resets the stall watchdog (see `PingTimeout`).
    pub(in crate::core::app) fn apply_download_progress(
        &mut self,
        progress: &super_tts_shared::models::protocol::DownloadProgress,
    ) {
        self.last_switch_progress_at = Some(std::time::Instant::now());
        let target_model = progress.model_name.clone();
        match progress.status.as_str() {
            "loading_model" => {
                self.set_model_loading_with(
                    target_model,
                    "Loading model into memory...".to_string(),
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
                self.model_operation_state = ModelOperationState::Error { message };
                // A failed switch leaves the daemon idle — clear the selection
                // so the UI doesn't show a model that isn't loaded (mirrors the
                // `ModelError` handler).
                self.clear_loaded_model();
            }
            "completed" | "cancelled" => {
                // State will be updated by subsequent daemon events
                log::info!("Download finished with status: {}", progress.status);
            }
            _ => {
                // "verifying", "downloading", and anything the daemon adds
                // later: files are being provisioned.
                self.set_model_provisioning(target_model, progress.clone());
            }
        }
    }

    /// Model-switch stall watchdog, called on each `PingTimeout` tick. While a
    /// switch is in flight, a progress event (download tick, `loading_model`,
    /// or the initial `set_model_loading`) resets the clock; if none arrives
    /// within [`SWITCH_STALL_TIMEOUT`], flip to an error so the UI doesn't
    /// spin forever. No-op outside a switch.
    pub(in crate::core::app) fn check_switch_stall(&mut self) {
        if !matches!(
            self.model_operation_state,
            ModelOperationState::Loading { .. } | ModelOperationState::Provisioning { .. }
        ) {
            return;
        }
        if let Some(since) = self.last_switch_progress_at
            && since.elapsed() > SWITCH_STALL_TIMEOUT
        {
            log::warn!(
                "Model switch stalled: no progress for {}s",
                SWITCH_STALL_TIMEOUT.as_secs()
            );
            self.model_operation_state = ModelOperationState::Error {
                message: "Model switch stalled — the daemon stopped reporting progress."
                    .to_string(),
            };
            self.last_switch_progress_at = None;
        }
    }

    /// Updates the header and window titles.
    pub(in crate::core::app) fn update_title(&mut self) -> Task<cosmic::Action<Message>> {
        let mut window_title = "Super TTS".to_string();

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
