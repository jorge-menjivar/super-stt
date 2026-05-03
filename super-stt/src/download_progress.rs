// SPDX-License-Identifier: GPL-3.0-only
use chrono::Utc;
use log::{info, warn};
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use super_stt_shared::NotificationManager;
use super_stt_shared::models::protocol::DownloadProgress;
use tokio::sync::mpsc;

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
    pub notification_manager: Option<Arc<NotificationManager>>,
    last_broadcast_percentage: AtomicU64, // Store as fixed point (percentage * 100)
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
            notification_manager: None,
            last_broadcast_percentage: AtomicU64::new(0),
        }
    }

    #[must_use]
    pub fn with_notification_manager(mut self, nm: Arc<NotificationManager>) -> Self {
        self.notification_manager = Some(nm);
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

    /// Broadcast progress update via notification system
    pub async fn broadcast_progress(&self) {
        let progress = self.get_progress();

        // Only broadcast at 1% intervals to avoid flooding
        // Clamp, round and convert to a fixed-point integer (percentage * 100)
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let current_percentage = (progress.percentage.clamp(0.0, 100.0) * 100.0).round() as u64;
        let last_percentage = self.last_broadcast_percentage.load(Ordering::Relaxed);

        if current_percentage > last_percentage && current_percentage - last_percentage >= 100 {
            self.last_broadcast_percentage
                .store(current_percentage, Ordering::Relaxed);

            if let Some(ref nm) = self.notification_manager {
                let _ = nm
                    .broadcast_event(
                        "download_progress".to_string(),
                        "daemon".to_string(),
                        serde_json::json!({
                            "model_name": progress.model_name,
                            "current_file": progress.current_file,
                            "file_index": progress.file_index,
                            "total_files": progress.total_files,
                            "bytes_downloaded": progress.bytes_downloaded,
                            "total_bytes": progress.total_bytes,
                            "percentage": progress.percentage,
                            "status": progress.status,
                            "eta_seconds": progress.eta_seconds,
                            "timestamp": Utc::now().to_rfc3339()
                        }),
                    )
                    .await;
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
