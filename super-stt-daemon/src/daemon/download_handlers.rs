// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::device_management::PipelineStage;
use crate::daemon::types::SuperSTTDaemon;
use log::{info, warn};
use super_stt_shared::models::protocol::{DaemonResponse, ErrorCode};

impl SuperSTTDaemon {
    /// Abandon the download `stage` has in flight.
    ///
    /// Scoped to the stage that asked: the stages provision independently, and
    /// cancelling a load the caller is not looking at would be a surprise. A
    /// stage with nothing of its own in flight gets `no_switch_in_progress`,
    /// whether or not another stage is downloading.
    #[must_use]
    pub fn handle_cancel_download(&self, stage: PipelineStage) -> DaemonResponse {
        match self.download_manager.cancel_download(stage.position()) {
            Ok(()) => {
                info!(
                    "Download cancellation requested for stage {}",
                    stage.position()
                );
                DaemonResponse::success()
                    .with_message("Download cancelled successfully".to_string())
            }
            Err(e) => {
                // The sole failure is "nothing to cancel" — a state conflict
                // (409 `no_switch_in_progress`), not a server error.
                warn!("Failed to cancel download: {e}");
                DaemonResponse::error_with_code(ErrorCode::NoSwitchInProgress, &e)
            }
        }
    }

    /// The download `stage` has in flight, if any.
    #[must_use]
    pub fn handle_get_download_status(&self, stage: PipelineStage) -> DaemonResponse {
        if let Some(tracker) = self.download_manager.get_download(stage.position()) {
            let progress = tracker.get_progress();
            DaemonResponse::success().with_download_progress(progress)
        } else {
            DaemonResponse::success().with_message("No download in progress".to_string())
        }
    }
}
