// SPDX-License-Identifier: GPL-3.0-only
//! Model-file provisioning for backends: `super_engine_daemon::download`,
//! shared with Super STT.
//!
//! Backends declare the files they need in `backend.toml` (`[[models.files]]`).
//! Each file is a plain URL plus a `destination` path; the daemon downloads it
//! into the per-backend directory before spawning the backend, so a sandboxed
//! backend never needs network access of its own.

use std::sync::Arc;

use anyhow::Result;
pub use super_engine_daemon::download::DownloadItem;

use crate::download_progress::DownloadProgressTracker;

/// Download a model's files into the backend directory, reporting through
/// `tracker` when present. See `super_engine_daemon::download::download_files`.
///
/// # Errors
///
/// Returns an error on network/IO failure, a non-success HTTP status, a
/// SHA-256 mismatch, or cancellation via `tracker.is_cancelled()`.
pub async fn download_files(
    items: &[DownloadItem],
    tracker: Option<&Arc<DownloadProgressTracker>>,
    starting_file_index: usize,
) -> Result<()> {
    super_engine_daemon::download::download_files(
        items,
        tracker,
        starting_file_index,
        super_tts_registry_types::Tts::USER_AGENT,
    )
    .await
}
