// SPDX-License-Identifier: GPL-3.0-only
//! What a model load reports as it provisions the model's files:
//! `super_engine_daemon::download_progress`, shared with Super STT. Super TTS
//! has one load at a time, so its slot is `()`.

use super_engine_daemon::download_progress as engine;
use super_tts_shared::models::protocol::DownloadProgress;

pub use super_engine_daemon::download_progress::{Progress, status};

/// Progress of one model load. See
/// `super_engine_daemon::download_progress::DownloadProgressTracker`.
pub type DownloadProgressTracker = engine::DownloadProgressTracker<()>;

/// The one load in flight. See
/// `super_engine_daemon::download_progress::DownloadStateManager`.
pub type DownloadStateManager = engine::DownloadStateManager<()>;

/// A load's progress as the wire carries it.
#[must_use]
pub fn report(progress: Progress<()>) -> DownloadProgress {
    DownloadProgress {
        model_name: progress.model_name,
        current_file: progress.current_file,
        file_index: progress.file_index,
        total_files: progress.total_files,
        bytes_downloaded: progress.bytes_downloaded,
        total_bytes: progress.total_bytes,
        percentage: progress.percentage,
        status: progress.status,
        started_at: progress.started_at,
        eta_seconds: progress.eta_seconds,
        error: progress.error,
        load: progress.load,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;

    /// The event the engine publishes reads back as the wire type a client
    /// parses, and carries what the typed report does.
    #[test]
    fn the_published_event_is_the_wire_type() {
        let tracker = DownloadProgressTracker::new(
            "kokoro-82m".to_string(),
            (),
            2,
            Arc::new(AtomicBool::new(false)),
        );
        tracker.mark_error("disk full");

        let wire = report(tracker.get_progress());
        assert_eq!(wire.model_name, "kokoro-82m");
        assert_eq!(wire.total_files, 2);
        assert_eq!(wire.error.as_deref(), Some("disk full"));

        let event = serde_json::to_value(tracker.get_progress()).unwrap();
        let parsed: DownloadProgress =
            serde_json::from_value(event).expect("the published event is the wire type");
        assert_eq!(parsed.model_name, wire.model_name);
        assert_eq!(parsed.status, "error");
        assert_eq!(parsed.error, wire.error);
    }
}
