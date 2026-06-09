// SPDX-License-Identifier: GPL-3.0-only
//! Model-file provisioning for backends.
//!
//! Backends declare the files they need in `backend.toml` (`[[models.files]]`).
//! The daemon downloads those files into the per-backend directory before
//! spawning the backend, so a sandboxed backend never needs network access of
//! its own. This is the only downloader the daemon keeps now that model
//! inference lives entirely in out-of-tree backends.

use anyhow::Result;
use futures::StreamExt;
use log::info;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::download_progress::DownloadProgressTracker;

/// Build the `HuggingFace` Hub download URL for a single file.
fn hf_url(repo: &str, revision: &str, filename: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/{revision}/{filename}")
}

/// Download a model's files from `HuggingFace` into a flat directory.
///
/// Used to provision a backend's per-backend model directory (the manifest
/// `dest`). Skips files already present (non-zero size) and writes plain
/// `dest_dir/<filename>` files so a sandboxed backend can load them directly.
///
/// When `tracker` is `Some`, per-file and per-byte progress is reported
/// through it (`start_file` → `bytes_downloaded`/`total_bytes` → broadcast). When
/// `None`, downloads run silently (used by unit tests and one-off calls
/// that don't go through the daemon's `DownloadStateManager`).
///
/// `starting_file_index` lets the caller compose multiple
/// `download_files_to_dir` calls against a single tracker so the file
/// counter stays monotonic — pass `0` for the first call and the running
/// total for subsequent ones.
///
/// # Errors
///
/// Returns an error on network/IO failure, a non-success HTTP status, or
/// cancellation via `tracker.is_cancelled()`.
pub async fn download_files_to_dir(
    repo: &str,
    revision: &str,
    files: &[String],
    dest_dir: &Path,
    tracker: Option<&Arc<DownloadProgressTracker>>,
    starting_file_index: usize,
) -> Result<()> {
    fs::create_dir_all(dest_dir).await?;
    crate::install_crypto_provider();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_hours(1))
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()?;

    for (offset, filename) in files.iter().enumerate() {
        if let Some(t) = tracker
            && t.is_cancelled()
        {
            anyhow::bail!("download cancelled");
        }

        let file_index = starting_file_index + offset;
        let dest = dest_dir.join(filename);
        if let Ok(md) = fs::metadata(&dest).await
            && md.len() > 0
        {
            info!("Already present: {}", dest.display());
            // Reflect this file as "done" in the tracker. Per-file
            // counters: both `total_bytes` and `bytes_downloaded` are
            // this file's size (so the UI shows "X.X / X.X MB" at
            // 100%), then the file_index advances on the next iteration.
            if let Some(t) = tracker {
                t.start_file(filename, file_index);
                t.bytes_downloaded.store(md.len(), Ordering::Relaxed);
                t.total_bytes.store(md.len(), Ordering::Relaxed);
                t.broadcast_progress();
            }
            continue;
        }

        let url = hf_url(repo, revision, filename);
        info!("Downloading {url} -> {}", dest.display());
        if let Some(t) = tracker {
            t.start_file(filename, file_index);
        }
        let response = client.get(&url).send().await?;
        if !response.status().is_success() {
            anyhow::bail!("Download failed with status {}: {url}", response.status());
        }

        // Resolve the total file size so the progress bar fills smoothly.
        //
        // HuggingFace's CDN serves large `.safetensors` with chunked
        // transfer encoding, so `Content-Length` is often missing — but HF
        // sets a custom `X-Linked-Size` header on the resolve endpoint that
        // gives the underlying file size. After reqwest follows the
        // CDN redirect, the *final* response's headers are what we see,
        // and the CDN passes `X-Linked-Size` through. If that's missing
        // too, we make an explicit HEAD against the resolve URL — HF's
        // HEAD reliably returns `Content-Length`.
        let resolved_size = if tracker.is_some() {
            let from_get = response
                .headers()
                .get("x-linked-size")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| response.content_length());
            let size = if let Some(n) = from_get {
                Some(n)
            } else {
                // Fall back to a HEAD: same URL, redirects followed,
                // and check both `X-Linked-Size` and `Content-Length`.
                let head = client.head(&url).send().await.ok();
                head.as_ref().and_then(|r| {
                    r.headers()
                        .get("x-linked-size")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .or_else(|| r.content_length())
                })
            };
            match size {
                Some(n) => info!("Resolved size for {filename}: {n} bytes"),
                None => info!(
                    "No size header for {filename} (X-Linked-Size + Content-Length absent on GET and HEAD); progress will only update at file boundaries"
                ),
            }
            size
        } else {
            None
        };
        if let Some(t) = tracker
            && let Some(len) = resolved_size
        {
            // Per-file size, not an aggregate — `start_file` already
            // zeroed `total_bytes` and `bytes_downloaded` for this
            // file, so a plain `store` is what we want here.
            t.total_bytes.store(len, Ordering::Relaxed);
            // Broadcast immediately so the UI's MB display flips from
            // "0.0 / 0.0 MB" to the real total before the first chunk
            // lands. `broadcast_progress` detects the `total_bytes`
            // change and bypasses its 1%-percentage throttle.
            t.broadcast_progress();
        }

        let tmp = dest_dir.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
        let mut file = fs::File::create(&tmp).await?;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            if let Some(t) = tracker
                && t.is_cancelled()
            {
                anyhow::bail!("download cancelled");
            }
            let bytes = chunk?;
            file.write_all(&bytes).await?;
            if let Some(t) = tracker {
                t.bytes_downloaded
                    .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                // The tracker itself throttles to 1% increments, so calling
                // every chunk is fine — the SSE subscribers won't see a flood.
                t.broadcast_progress();
            }
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        fs::rename(&tmp, &dest).await?;
    }

    Ok(())
}
