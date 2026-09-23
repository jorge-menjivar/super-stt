// SPDX-License-Identifier: GPL-3.0-only
//! What a model load reports as it provisions the model's files:
//! `super_engine_daemon::download_progress`, shared with Super TTS. A Super
//! STT load is keyed by the pipeline stage it provisions, which a
//! post-processor and a transcription model do independently.

use serde::Serialize;
use super_engine_daemon::download_progress as engine;
use super_stt_shared::models::protocol::DownloadProgress;

pub use super_engine_daemon::download_progress::{Progress, status};

/// Which load a tracker is for: the backend serving the model, and the
/// `/pipeline/{stage}` position provisioning it. Reported on every tick so a
/// client can put the progress on the card that started it: the model name
/// alone does not say whose download it is, and both stages download through
/// the same tracker.
#[derive(Clone, Debug, Serialize)]
pub struct StageSlot {
    pub source: String,
    pub stage: u32,
}

impl engine::Slot for StageSlot {
    type Key = u32;

    fn key(&self) -> u32 {
        self.stage
    }

    fn describe(stage: u32) -> Option<String> {
        Some(format!("stage {stage}"))
    }
}

/// Progress of one model load. See
/// `super_engine_daemon::download_progress::DownloadProgressTracker`.
pub type DownloadProgressTracker = engine::DownloadProgressTracker<StageSlot>;

/// The loads in flight, one per pipeline stage. See
/// `super_engine_daemon::download_progress::DownloadStateManager`.
pub type DownloadStateManager = engine::DownloadStateManager<StageSlot>;

/// A load's progress as the wire carries it.
#[must_use]
pub fn report(progress: Progress<StageSlot>) -> DownloadProgress {
    DownloadProgress {
        model_name: progress.model_name,
        source: progress.slot.source,
        stage: progress.slot.stage,
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
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use super_stt_shared::models::protocol::POST_PROCESSOR_STAGE;

    /// The reported bug: a post-processor's download reported progress a
    /// client could only attribute by looking its model name up in the
    /// catalog, which put the progress bar on the transcription card. Every
    /// tick says which stage and which backend it is for — in the typed
    /// report and in the event the engine publishes, which must read back as
    /// the same wire type.
    #[test]
    fn progress_reports_the_stage_and_backend_it_is_for() {
        let tracker = DownloadProgressTracker::new(
            "s1-mini-q4_k_m".to_string(),
            StageSlot {
                source: "github.com/super-stt/s1-mini".to_string(),
                stage: POST_PROCESSOR_STAGE,
            },
            2,
            Arc::new(AtomicBool::new(false)),
        );
        tracker.mark_error("disk full");

        let wire = report(tracker.get_progress());
        assert_eq!(wire.model_name, "s1-mini-q4_k_m");
        assert_eq!(wire.source, "github.com/super-stt/s1-mini");
        assert_eq!(wire.stage, POST_PROCESSOR_STAGE);
        assert_eq!(wire.error.as_deref(), Some("disk full"));

        let event = serde_json::to_value(tracker.get_progress()).unwrap();
        let parsed: DownloadProgress =
            serde_json::from_value(event).expect("the published event is the wire type");
        assert_eq!(parsed.source, wire.source);
        assert_eq!(parsed.stage, wire.stage);
        assert_eq!(parsed.status, "error");
        assert_eq!(parsed.error, wire.error);
    }
}
