// SPDX-License-Identifier: GPL-3.0-only

use crate::ui::messages::Message;
use cosmic::prelude::*;

use super::{AppModel, DeviceState, ModelOperationState};

impl AppModel {
    /// Check if model is ready
    pub fn is_model_ready(&self) -> bool {
        matches!(self.model_operation_state, ModelOperationState::Ready)
    }

    /// Set model to downloading state
    pub(in crate::core::app) fn set_model_downloading(
        &mut self,
        target_model: String,
        progress: super_stt_shared::models::protocol::DownloadProgress,
    ) {
        self.model_operation_state = ModelOperationState::Downloading {
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
        self.model_operation_state = ModelOperationState::Loading {
            target_model,
            status_message,
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
    /// - `"completed" | "cancelled" | "error"` → no state change (terminal
    ///   events will arrive separately via daemon status events)
    /// - anything else (`"downloading"`, …) → `Downloading` state
    pub(in crate::core::app) fn apply_download_progress(
        &mut self,
        progress: &super_stt_shared::models::protocol::DownloadProgress,
    ) {
        let target_model = progress.model_name.clone();
        match progress.status.as_str() {
            "loading_model" => {
                self.set_model_loading(target_model, "Loading model into memory...".to_string());
            }
            "completed" | "cancelled" | "error" => {
                // State will be updated by subsequent daemon events
                log::info!("Download completed with status: {}", progress.status);
            }
            _ => {
                // "downloading" and other states default to downloading
                self.set_model_downloading(target_model, progress.clone());
            }
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
