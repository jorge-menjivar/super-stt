// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use log::{info, warn};
use super_stt_shared::models::protocol::{DaemonResponse, ErrorCode};

impl SuperSTTDaemon {
    /// Handle cancel download command
    #[must_use]
    pub fn handle_cancel_download(&self) -> DaemonResponse {
        match self.download_manager.cancel_current_download() {
            Ok(()) => {
                info!("Download cancellation requested");
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

    /// Handle get download status command
    #[must_use]
    pub fn handle_get_download_status(&self) -> DaemonResponse {
        if let Some(tracker) = self.download_manager.get_current_download() {
            let progress = tracker.get_progress();
            DaemonResponse::success().with_download_progress(progress)
        } else {
            DaemonResponse::success().with_message("No download in progress".to_string())
        }
    }
}
