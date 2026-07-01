// SPDX-License-Identifier: GPL-3.0-only
use chrono::Utc;
use log::{info, warn};
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use super_stt_shared::models::protocol::DownloadProgress;

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
    /// Failure detail set by [`Self::mark_error`]; included in the
    /// `download_progress` payload so a client can show why a switch failed.
    pub error: Arc<RwLock<Option<String>>>,
    pub started_at: Instant,
    pub started_at_str: String,
    pub cancelled: Arc<AtomicBool>,
    /// Optional event bus the tracker publishes `download_progress`
    /// SSE events into. Set via [`Self::with_event_bus`]; when `None`
    /// the HTTP `/events` channel sees nothing.
    pub events: Option<Arc<EventBus>>,
    last_broadcast_percentage: AtomicU64, // Store as fixed point (percentage * 100)
    /// The `status` string we most recently broadcast. Used so that
    /// transitions to "completed" / "cancelled" / "error" are emitted
    /// even when the 1%-increment percentage gate would otherwise
    /// suppress them.
    last_broadcast_status: Arc<RwLock<String>>,
    /// The `total_bytes` value we most recently broadcast. A change
    /// here (typically 0 → file size, once we resolve it from
    /// `X-Linked-Size` / `Content-Length` / HEAD) must publish even
    /// when neither percentage nor status changed — otherwise the UI's
    /// "x.x / y.y MB" line stays at "0.0 / 0.0 MB" until enough bytes
    /// stream for the percentage to cross a 1% boundary, which for a
    /// multi-GB file can be several seconds of frozen UI.
    last_broadcast_total_bytes: AtomicU64,
    /// The `file_index` we most recently broadcast. File transitions
    /// (per-file `total_bytes`/`bytes_downloaded` reset, percentage
    /// dropping from ~90% back to 0%) must publish even when the new
    /// file happens to be the same size as the previous one — the
    /// percentage drop alone is *not* caught by `percentage_crossed`
    /// (which only fires on increases). Initialized to `usize::MAX`
    /// so the very first broadcast also fires.
    last_broadcast_file_index: AtomicUsize,
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
            error: Arc::new(RwLock::new(None)),
            started_at: Instant::now(),
            started_at_str: Utc::now().to_rfc3339(),
            cancelled,
            events: None,
            last_broadcast_percentage: AtomicU64::new(0),
            last_broadcast_status: Arc::new(RwLock::new(String::new())),
            last_broadcast_total_bytes: AtomicU64::new(0),
            last_broadcast_file_index: AtomicUsize::new(usize::MAX),
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

    pub fn get_progress(&self) -> DownloadProgress {
        let bytes_downloaded = self.bytes_downloaded.load(Ordering::Relaxed);
        let total_bytes = self.total_bytes.load(Ordering::Relaxed);

        let status = self.status.read().clone();
        let percentage: f32 = if status == "completed" || status == "loading_model" {
            // Files are all on disk. The remaining work (spawning the
            // backend, loading weights onto the device) isn't
            // byte-tracked, so keep the bar full — the app shows a
            // separate "Loading…" indicator for the `loading_model`
            // phase.
            100.0
        } else if total_bytes > 0 {
            // Per-file progress: how much of the *current* file has
            // arrived, matching the per-file "X.X / Y.Y MB" readout.
            // `bytes_downloaded`/`total_bytes` reset at each file
            // boundary (`start_file`), so the bar fills 0→100% per file
            // and the last file's final chunk fills it to the end. No
            // 90% cap: the old in-tree flow reserved 90–100% for a
            // loading phase, but subprocess backends report that phase
            // separately via the `loading_model` status above. The
            // `file_index` counter ("2/4") conveys which file is in
            // flight.
            crate::num_cast::f64_to_f32(
                (crate::num_cast::u64_to_f64(bytes_downloaded)
                    / crate::num_cast::u64_to_f64(total_bytes)
                    * 100.0)
                    .min(100.0),
            )
        } else {
            // Size for the current file not resolved yet (or there's
            // nothing to download).
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
            error: self.error.read().clone(),
        }
    }

    /// Broadcast progress update via the HTTP `/events` SSE bus. Throttled to 1%
    /// increments so a tight streaming download doesn't flood
    /// subscribers — but any transition to a terminal `status` value
    /// (`completed` / `cancelled` / `error`) always publishes, since
    /// the consumer's `if progress.status == "completed"` arm clears
    /// the in-UI download indicator and a dropped event there leaves
    /// the UI permanently stuck.
    pub fn broadcast_progress(&self) {
        let progress = self.get_progress();

        // Clamp, round and convert to a fixed-point integer (percentage * 100)
        let current_percentage =
            crate::num_cast::f32_to_u64((progress.percentage.clamp(0.0, 100.0) * 100.0).round());
        let last_percentage = self.last_broadcast_percentage.load(Ordering::Relaxed);

        let status_changed = {
            let last = self.last_broadcast_status.read().clone();
            last != progress.status
        };
        let percentage_crossed =
            current_percentage > last_percentage && current_percentage - last_percentage >= 100;
        // Newly-resolved file size — broadcast immediately so the UI's
        // "x.x / y.y MB" line flips from "0.0 / 0.0 MB" to the real total
        // before the first chunk's percentage update lands.
        let total_bytes_changed =
            self.last_broadcast_total_bytes.load(Ordering::Relaxed) != progress.total_bytes;
        // File boundary — per-file counters reset at the start of the
        // next file, so we publish even if the new file's size matches
        // the previous file's (which would otherwise leave every
        // throttle arm unchanged).
        let file_index_changed =
            self.last_broadcast_file_index.load(Ordering::Relaxed) != progress.file_index;

        if status_changed || percentage_crossed || total_bytes_changed || file_index_changed {
            self.last_broadcast_percentage
                .store(current_percentage, Ordering::Relaxed);
            self.last_broadcast_status
                .write()
                .clone_from(&progress.status);
            self.last_broadcast_total_bytes
                .store(progress.total_bytes, Ordering::Relaxed);
            self.last_broadcast_file_index
                .store(progress.file_index, Ordering::Relaxed);

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
    }

    pub fn start_file(&self, filename: &str, file_index: usize) {
        *self.current_file.write() = filename.to_string();
        self.file_index.store(file_index, Ordering::Relaxed);
        // Per-file counters: `total_bytes` and `bytes_downloaded`
        // reflect only the current file, so each file's UI display
        // is "X.X / <this file's size> MB" rather than an aggregate
        // across the whole model (which is confusing when sizes
        // vary by orders of magnitude — `config.json` is sub-MB,
        // `model-*.safetensors` is multi-GB). The caller publishes
        // the new file's real numbers after this returns: cached
        // files store `(md.len(), md.len())` directly, fresh
        // downloads resolve size via `X-Linked-Size`/HEAD and store
        // `(file_size, 0)`. Until then we're at 0/0, but no
        // broadcast goes out between this reset and the caller's
        // store, so the UI never observes the transient.
        self.bytes_downloaded.store(0, Ordering::Relaxed);
        self.total_bytes.store(0, Ordering::Relaxed);
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

    /// Mark the file-download phase done and the (untracked) weight-load
    /// phase begun. The settings app maps this status to a "Loading model
    /// into memory…" indicator, so the user sees the operation is still
    /// progressing after the download bar fills.
    pub fn mark_loading(&self) {
        *self.status.write() = "loading_model".to_string();
        info!(
            "Files downloaded; loading model into memory: {}",
            self.model_name
        );
    }

    pub fn mark_completed(&self) {
        *self.status.write() = "completed".to_string();
        info!("Download completed for model: {}", self.model_name);
    }

    pub fn mark_error(&self, error: &str) {
        *self.status.write() = "error".to_string();
        *self.error.write() = Some(error.to_string());
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

    /// Regression for the "0.0 / 0.0 MB" UI bug: when a download starts
    /// with `total_bytes = 0` (HF's CDN serves large `.safetensors`
    /// chunked, so no `Content-Length`), the first broadcast publishes
    /// `total_bytes = 0`. Once the daemon resolves the real size via
    /// `X-Linked-Size`/HEAD, it bumps `total_bytes` and calls
    /// `broadcast_progress()` — but `bytes_downloaded` is still 0, so
    /// percentage stays 0% and status stays "downloading". Without
    /// detecting the `total_bytes` change, the throttle would suppress
    /// the publish and the UI would freeze on "0.0 / 0.0 MB" until
    /// enough chunks streamed to cross the 1% gate (several seconds on
    /// a slow connection for a multi-GB file).
    #[test]
    fn broadcast_progress_publishes_when_total_bytes_changes() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let tracker = DownloadProgressTracker::new("test-model".to_string(), 1, cancelled);

        // Initial broadcast — empty status → "downloading", total_bytes = 0.
        tracker.broadcast_progress();
        assert_eq!(
            tracker.last_broadcast_total_bytes.load(Ordering::Relaxed),
            0
        );

        // Size resolves (4 GB). bytes_downloaded still 0, status
        // unchanged, percentage still 0%. The publish must fire because
        // total_bytes changed.
        tracker.total_bytes.store(4_000_000_000, Ordering::Relaxed);
        tracker.broadcast_progress();
        assert_eq!(
            tracker.last_broadcast_total_bytes.load(Ordering::Relaxed),
            4_000_000_000,
            "broadcast must fire when total_bytes flips from 0 to the resolved file size"
        );
    }

    /// Per-file counters: `start_file` zeroes `total_bytes` and
    /// `bytes_downloaded` so the UI's "X.X / Y.Y MB" displays only
    /// the current file's size, not an aggregate across the whole
    /// model. Without this, a multi-file model (e.g. Voxtral with two
    /// 3GB safetensors plus a config) shows a cumulative "1500 /
    /// 6000 MB" mid-second-file, which is confusing.
    #[test]
    fn start_file_resets_per_file_counters() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let tracker = DownloadProgressTracker::new("test-model".to_string(), 2, cancelled);

        // Simulate file 0 finishing.
        tracker.total_bytes.store(3_000_000_000, Ordering::Relaxed);
        tracker
            .bytes_downloaded
            .store(3_000_000_000, Ordering::Relaxed);

        // Starting file 1 must zero out per-file counters so the next
        // size resolution / chunk doesn't accumulate on top of file 0.
        tracker.start_file("model-00002-of-00002.safetensors", 1);
        assert_eq!(tracker.total_bytes.load(Ordering::Relaxed), 0);
        assert_eq!(tracker.bytes_downloaded.load(Ordering::Relaxed), 0);
        assert_eq!(tracker.file_index.load(Ordering::Relaxed), 1);
    }

    /// File-index transitions must publish even when the overall
    /// percentage is flat across the boundary — which is precisely the
    /// common case with overall (not per-file) progress: file N
    /// finishing at `(N+1)/total` and file N+1 starting at
    /// `(N+1)/total` are the same number. Neither `percentage_crossed`
    /// nor `total_bytes_changed` (same-size files) fires, so without the
    /// `file_index` arm the throttle would suppress the publish and the
    /// per-file "X / Y MB" display would stay frozen on the old file.
    #[test]
    fn broadcast_progress_publishes_when_file_index_advances() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let tracker = DownloadProgressTracker::new("test-model".to_string(), 2, cancelled);

        // File 0 fully downloaded: overall = (0 + 1)/2 = 50%.
        tracker.total_bytes.store(1000, Ordering::Relaxed);
        tracker.bytes_downloaded.store(1000, Ordering::Relaxed);
        tracker.broadcast_progress();
        assert_eq!(tracker.last_broadcast_file_index.load(Ordering::Relaxed), 0);

        // File 1 starts at 0 bytes, same size: overall = (1 + 0)/2 =
        // 50% — identical to the previous broadcast. Only file_index
        // moved.
        tracker.start_file("file2", 1);
        tracker.total_bytes.store(1000, Ordering::Relaxed);
        tracker.broadcast_progress();
        assert_eq!(
            tracker.last_broadcast_file_index.load(Ordering::Relaxed),
            1,
            "file_index update must publish even when overall percentage is flat across the boundary"
        );
    }

    /// The per-file bar reaches 100% when the current file is fully
    /// downloaded — no 90% cap left over from the in-tree flow. (The
    /// last file hitting 100%, then `loading_model`/`completed`, fills
    /// the bar to the end of the whole operation.)
    #[test]
    fn percentage_reaches_100_when_current_file_complete() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let tracker = DownloadProgressTracker::new("m".to_string(), 3, cancelled);
        tracker.start_file("f3", 2);
        tracker.total_bytes.store(500, Ordering::Relaxed);
        tracker.bytes_downloaded.store(500, Ordering::Relaxed);
        let pct = tracker.get_progress().percentage;
        assert!((pct - 100.0).abs() < 0.01, "expected 100%, got {pct}");
    }

    /// Per-file bar: progress is the current file's byte fraction, not
    /// an aggregate across files. A half-downloaded second file reads
    /// 50% regardless of how large the already-finished first file was.
    #[test]
    fn percentage_is_per_file_not_aggregate() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let tracker = DownloadProgressTracker::new("m".to_string(), 2, cancelled);
        // Second file (index 1 of 2), half downloaded.
        tracker.start_file("f2", 1);
        tracker.total_bytes.store(1000, Ordering::Relaxed);
        tracker.bytes_downloaded.store(500, Ordering::Relaxed);
        let pct = tracker.get_progress().percentage;
        assert!(
            (pct - 50.0).abs() < 0.01,
            "expected per-file 50%, got {pct}"
        );
    }

    /// `loading_model` and `completed` both pin the bar at 100% — the
    /// post-download weight-load phase keeps the bar full while the app
    /// shows its "Loading…" indicator.
    #[test]
    fn loading_and_completed_statuses_are_100_percent() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let tracker = DownloadProgressTracker::new("m".to_string(), 2, cancelled);
        tracker.start_file("f1", 0);
        tracker.total_bytes.store(500, Ordering::Relaxed);
        tracker.bytes_downloaded.store(100, Ordering::Relaxed);

        tracker.mark_loading();
        assert!((tracker.get_progress().percentage - 100.0).abs() < 0.01);

        tracker.mark_completed();
        assert!((tracker.get_progress().percentage - 100.0).abs() < 0.01);
    }

    /// Companion of the tests above: when nothing meaningful changes
    /// (percentage, status, total_bytes, file_index all stable), the
    /// throttle suppresses the publish. Without this guard,
    /// broadcast_progress would spam events on every chunk.
    #[test]
    fn broadcast_progress_suppressed_when_neither_percentage_nor_status_changes() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let tracker = DownloadProgressTracker::new("test-model".to_string(), 1, cancelled);
        tracker.total_bytes.store(1000, Ordering::Relaxed);
        tracker.bytes_downloaded.store(500, Ordering::Relaxed);

        // First broadcast — overall progress of the sole file at
        // 500/1000 = 50%, fixed-point 5000.
        tracker.broadcast_progress();
        let after_first = tracker.last_broadcast_percentage.load(Ordering::Relaxed);
        assert_eq!(after_first, 5000);

        // Same percentage, same status, same file_index → no change.
        tracker.broadcast_progress();
        assert_eq!(
            tracker.last_broadcast_percentage.load(Ordering::Relaxed),
            5000,
            "second call with identical state must be a no-op"
        );
    }
}
