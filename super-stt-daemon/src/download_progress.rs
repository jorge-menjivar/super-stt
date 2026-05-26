// SPDX-License-Identifier: GPL-3.0-only
use chrono::Utc;
use log::{info, warn};
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use super_stt_shared::models::protocol::DownloadProgress;
use tokio::sync::mpsc;

use crate::daemon::events::EventBus;

/// Progress tracker for model downloads that implements the hf-hub Progress trait
pub struct DownloadProgressTracker {
    pub model_name: String,
    pub current_file: Arc<RwLock<String>>,
    pub file_index: AtomicUsize,
    pub total_files: AtomicUsize,
    pub bytes_downloaded: AtomicU64,
    pub total_bytes: AtomicU64,
    pub status: Arc<RwLock<String>>,
    pub started_at: Instant,
    pub started_at_str: String,
    pub cancelled: Arc<AtomicBool>,
    pub progress_sender: Option<mpsc::UnboundedSender<DownloadProgress>>,
    /// Optional event bus the tracker publishes `download_progress`
    /// SSE events into. Set via [`Self::with_event_bus`]; when `None`
    /// the tracker still drives `progress_sender` but the HTTP
    /// `/events` channel sees nothing.
    pub events: Option<Arc<EventBus>>,
    last_broadcast_percentage: AtomicU64, // Store as fixed point (percentage * 100)
    /// The `status` string we most recently broadcast. Used so that
    /// transitions to "completed" / "cancelled" / "error" are emitted
    /// even when the 1%-increment percentage gate would otherwise
    /// suppress them.
    last_broadcast_status: Arc<RwLock<String>>,
}

impl DownloadProgressTracker {
    pub fn new(model_name: String, total_files: usize, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            model_name,
            current_file: Arc::new(RwLock::new(String::new())),
            file_index: AtomicUsize::new(0),
            total_files: AtomicUsize::new(total_files),
            bytes_downloaded: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            status: Arc::new(RwLock::new("downloading".to_string())),
            started_at: Instant::now(),
            started_at_str: Utc::now().to_rfc3339(),
            cancelled,
            progress_sender: None,
            events: None,
            last_broadcast_percentage: AtomicU64::new(0),
            last_broadcast_status: Arc::new(RwLock::new(String::new())),
        }
    }

    /// Attach the daemon's event bus so progress updates fan out as
    /// `download_progress` SSE events on `GET /events` (settings-scope
    /// only — see `super-stt-daemon/src/daemon/events.rs::Topic`).
    #[must_use]
    pub fn with_event_bus(mut self, events: Arc<EventBus>) -> Self {
        self.events = Some(events);
        self
    }

    #[must_use]
    pub fn with_progress_sender(mut self, sender: mpsc::UnboundedSender<DownloadProgress>) -> Self {
        self.progress_sender = Some(sender);
        self
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    pub fn get_progress(&self) -> DownloadProgress {
        let bytes_downloaded = self.bytes_downloaded.load(Ordering::Relaxed);
        let total_bytes = self.total_bytes.load(Ordering::Relaxed);

        // Calculate percentage based on file progress if byte-level tracking isn't available
        let file_index = self.file_index.load(Ordering::Relaxed);
        let total_files = self.total_files.load(Ordering::Relaxed);

        let status = self.status.read().clone();
        let percentage: f32 = if status == "loading_model" {
            // For model loading phase, show 95% to indicate almost complete
            95.0
        } else if total_bytes > 0 {
            // Use byte-level progress if available (capped at 90% for download phase)
            let pct = ((bytes_downloaded as f64 / total_bytes as f64) * 90.0).min(90.0);
            pct as f32
        } else if total_files > 0 {
            // Use file-based progress (capped at 90% for download phase)
            let completed_files = file_index.min(total_files) as f64;
            let pct = ((completed_files / total_files as f64) * 90.0).min(90.0);
            pct as f32
        } else {
            0.0
        };

        let elapsed = self.started_at.elapsed().as_secs();
        let eta_seconds = if bytes_downloaded > 0 && total_bytes > bytes_downloaded {
            let remaining_bytes = total_bytes - bytes_downloaded;
            let bytes_per_second = bytes_downloaded / elapsed.max(1);
            remaining_bytes.checked_div(bytes_per_second)
        } else {
            None
        };

        DownloadProgress {
            model_name: self.model_name.clone(),
            current_file: self.current_file.read().clone(),
            file_index: self.file_index.load(Ordering::Relaxed),
            total_files: self.total_files.load(Ordering::Relaxed),
            bytes_downloaded,
            total_bytes,
            percentage,
            status: self.status.read().clone(),
            started_at: self.started_at_str.clone(),
            eta_seconds,
        }
    }

    /// Broadcast progress update via the HTTP `/events` SSE bus and the
    /// optional in-process `progress_sender`. Throttled to 1%
    /// increments so a tight streaming download doesn't flood
    /// subscribers — but any transition to a terminal `status` value
    /// (`completed` / `cancelled` / `error`) always publishes, since
    /// the consumer's `if progress.status == "completed"` arm clears
    /// the in-UI download indicator and a dropped event there leaves
    /// the UI permanently stuck.
    pub fn broadcast_progress(&self) {
        let progress = self.get_progress();

        // Clamp, round and convert to a fixed-point integer (percentage * 100)
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let current_percentage = (progress.percentage.clamp(0.0, 100.0) * 100.0).round() as u64;
        let last_percentage = self.last_broadcast_percentage.load(Ordering::Relaxed);

        let status_changed = {
            let last = self.last_broadcast_status.read().clone();
            last != progress.status
        };
        let percentage_crossed =
            current_percentage > last_percentage && current_percentage - last_percentage >= 100;

        if status_changed || percentage_crossed {
            self.last_broadcast_percentage
                .store(current_percentage, Ordering::Relaxed);
            self.last_broadcast_status
                .write()
                .clone_from(&progress.status);

            if let Some(ref events) = self.events {
                let mut payload =
                    serde_json::to_value(&progress).unwrap_or_else(|_| serde_json::json!({}));
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert(
                        "timestamp".to_string(),
                        serde_json::Value::String(Utc::now().to_rfc3339()),
                    );
                }
                events.publish_download_progress(payload);
            }
        }

        // Also send via channel if available
        if let Some(ref sender) = self.progress_sender {
            let _ = sender.send(progress);
        }
    }

    pub fn start_file(&self, filename: &str, file_index: usize) {
        *self.current_file.write() = filename.to_string();
        self.file_index.store(file_index, Ordering::Relaxed);
        info!(
            "Downloading file {}/{}: {}",
            file_index + 1,
            self.total_files.load(Ordering::Relaxed),
            filename
        );
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        *self.status.write() = "cancelled".to_string();
        warn!("Download cancelled for model: {}", self.model_name);
    }

    pub fn mark_completed(&self) {
        *self.status.write() = "completed".to_string();
        info!("Download completed for model: {}", self.model_name);
    }

    pub fn mark_error(&self, error: &str) {
        *self.status.write() = "error".to_string();
        warn!("Download error for model {}: {}", self.model_name, error);
    }
}

/// Note: We're not implementing `hf_hub::api::Progress` directly since it's a private trait.
/// Instead, we use our own progress tracking system that integrates with the notification system.
/// Global download state manager
pub struct DownloadStateManager {
    current_download: Arc<RwLock<Option<Arc<DownloadProgressTracker>>>>,
    cancellation_flag: Arc<AtomicBool>,
}

impl Default for DownloadStateManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadStateManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            current_download: Arc::new(RwLock::new(None)),
            cancellation_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start tracking a download; fails if another download is active
    ///
    /// # Errors
    ///
    /// Returns an error if a download is already in progress.
    pub fn start_download(&self, tracker: Arc<DownloadProgressTracker>) -> Result<(), String> {
        let mut current = self.current_download.write();
        if current.is_some() {
            return Err("A download is already in progress".to_string());
        }
        *current = Some(tracker);
        self.cancellation_flag.store(false, Ordering::Relaxed);
        Ok(())
    }

    #[must_use]
    pub fn get_current_download(&self) -> Option<Arc<DownloadProgressTracker>> {
        self.current_download.read().clone()
    }

    /// Cancel the current download if present
    ///
    /// # Errors
    ///
    /// Returns an error if there is no active download to cancel.
    pub fn cancel_current_download(&self) -> Result<(), String> {
        let current = self.current_download.read();
        if let Some(ref tracker) = *current {
            tracker.cancel();
            self.cancellation_flag.store(true, Ordering::Relaxed);
            Ok(())
        } else {
            Err("No download in progress".to_string())
        }
    }

    pub fn clear_download(&self) {
        *self.current_download.write() = None;
        self.cancellation_flag.store(false, Ordering::Relaxed);
    }

    #[must_use]
    pub fn get_cancellation_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancellation_flag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for the silent UI-stuck bug: when the post-download
    /// `mark_completed()` runs and `broadcast_progress()` is called,
    /// the underlying computed percentage often isn't strictly greater
    /// than the last broadcast (it may even drop). The 1%-increment
    /// throttle alone would suppress the publish, and the consumer's
    /// `if progress.status == "completed"` arm would never fire — the
    /// "downloading" indicator would stay forever. Status transitions
    /// must bypass the throttle.
    #[test]
    fn broadcast_progress_publishes_completed_status_even_without_percentage_increase() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let tracker = DownloadProgressTracker::new("test-model".to_string(), 1, cancelled);
        tracker.total_bytes.store(1000, Ordering::Relaxed);
        tracker.bytes_downloaded.store(900, Ordering::Relaxed);

        // Simulate the throttle having already seen 90%.
        tracker
            .last_broadcast_percentage
            .store(9000, Ordering::Relaxed);
        *tracker.last_broadcast_status.write() = "downloading".to_string();

        // Now mark completed without bumping bytes_downloaded — so the
        // computed percentage stays at 90 — and confirm the broadcast
        // path still fired, evidenced by `last_broadcast_status`.
        *tracker.status.write() = "completed".to_string();
        tracker.broadcast_progress();

        assert_eq!(
            *tracker.last_broadcast_status.read(),
            "completed",
            "broadcast must fire on status transition to `completed` regardless of percentage"
        );
    }

    /// Companion of the test above: when neither percentage nor status
    /// changes, the throttle suppresses the publish. Without this
    /// guard, broadcast_progress would spam events.
    #[test]
    fn broadcast_progress_suppressed_when_neither_percentage_nor_status_changes() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let tracker = DownloadProgressTracker::new("test-model".to_string(), 1, cancelled);
        tracker.total_bytes.store(1000, Ordering::Relaxed);
        tracker.bytes_downloaded.store(500, Ordering::Relaxed);

        // First broadcast — percentage of 500/1000 capped at 90% =
        // 45%, fixed-point 4500.
        tracker.broadcast_progress();
        let after_first = tracker.last_broadcast_percentage.load(Ordering::Relaxed);
        assert_eq!(after_first, 4500);

        // Same percentage, same status → no change.
        tracker.broadcast_progress();
        assert_eq!(
            tracker.last_broadcast_percentage.load(Ordering::Relaxed),
            4500,
            "second call with identical state must be a no-op"
        );
    }
}
