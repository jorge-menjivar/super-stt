// SPDX-License-Identifier: GPL-3.0-only
//! Model-file provisioning for backends.
//!
//! Backends declare the files they need in `backend.toml` (`[[models.files]]`).
//! Each file is a plain URL plus a `destination` path; the daemon downloads it
//! into the per-backend directory before spawning the backend, so a sandboxed
//! backend never needs network access of its own. Files are fetched the same
//! way regardless of host — no source is given special treatment. This is the
//! only downloader the daemon keeps now that model inference lives entirely in
//! out-of-tree backends.

use crate::download_stream::{StreamError, stream_body_to_writer};
use anyhow::Result;
use log::info;
use ring::digest::{Context, SHA256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use super_stt_registry_types::verify::sha256_matches;
use tokio::fs;
use tokio::io::AsyncReadExt;

use crate::download_progress::DownloadProgressTracker;

/// One file to provision: a URL, the absolute path to write it to, and an
/// optional expected SHA-256 (hex) for integrity verification.
pub struct DownloadItem {
    /// Full download URL. Any host.
    pub url: String,
    /// Absolute path to write the file to (the caller has already joined the
    /// manifest `destination` onto the backend directory).
    pub destination: PathBuf,
    /// Expected SHA-256, hex-encoded, when the manifest declares one.
    pub sha256: Option<String>,
}

/// Hex-encoded SHA-256 of a file on disk, streamed so large weights don't load
/// into memory.
async fn sha256_hex_of_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).await?;
    let mut ctx = Context::new(&SHA256);
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Ok(hex::encode(ctx.finish().as_ref()))
}

/// If `dest` already holds a usable copy (non-empty, and matching `sha256` when
/// one is declared), returns its size. Returns `None` when the file is absent,
/// empty, or fails verification — in which case it should be re-downloaded.
async fn usable_existing(dest: &Path, sha256: Option<&str>) -> Result<Option<u64>> {
    let Ok(md) = fs::metadata(dest).await else {
        return Ok(None);
    };
    if md.len() == 0 {
        return Ok(None);
    }
    if let Some(expected) = sha256 {
        let actual = sha256_hex_of_file(dest).await?;
        if !sha256_matches(&actual, expected) {
            info!(
                "Hash mismatch for existing {} (expected {expected}, got {actual}); re-downloading",
                dest.display()
            );
            return Ok(None);
        }
    }
    Ok(Some(md.len()))
}

/// Best-effort total file size for the progress bar.
///
/// Some CDNs serve large files with chunked transfer encoding, so
/// `Content-Length` is often missing. Hugging Face, for one, sets a custom
/// `X-Linked-Size` header on its resolve endpoint with the underlying file
/// size; we read it first (simply absent, and harmless, on other hosts), then
/// fall back to `Content-Length`, then to an explicit HEAD.
async fn resolve_total_size(
    client: &reqwest::Client,
    response: &reqwest::Response,
    url: &str,
) -> Option<u64> {
    fn from_headers(h: &reqwest::header::HeaderMap) -> Option<u64> {
        h.get("x-linked-size")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
    }
    if let Some(n) = from_headers(response.headers()).or_else(|| response.content_length()) {
        return Some(n);
    }
    let head = client.head(url).send().await.ok()?;
    from_headers(head.headers()).or_else(|| head.content_length())
}

/// Download a single file to `item.destination`, verifying its SHA-256 when one
/// is declared and reporting progress through `tracker` when present.
async fn download_one(
    client: &reqwest::Client,
    item: &DownloadItem,
    tracker: Option<&Arc<DownloadProgressTracker>>,
    file_index: usize,
) -> Result<()> {
    let dest = &item.destination;
    let name = dest.file_name().map_or_else(
        || dest.to_string_lossy().into_owned(),
        |s| s.to_string_lossy().into_owned(),
    );
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).await?;

    // Skip a file that's already on disk (verified against `sha256` when set).
    // Per-file counters: both totals are this file's size so the UI shows
    // "X.X / X.X MB" at 100%, then the file_index advances next iteration.
    if let Some(len) = usable_existing(dest, item.sha256.as_deref()).await? {
        info!("Already present: {}", dest.display());
        if let Some(t) = tracker {
            t.start_file(&name, file_index);
            t.bytes_downloaded.store(len, Ordering::Relaxed);
            t.total_bytes.store(len, Ordering::Relaxed);
            t.broadcast_progress();
        }
        return Ok(());
    }

    let url = &item.url;
    info!("Downloading {url} -> {}", dest.display());
    if let Some(t) = tracker {
        t.start_file(&name, file_index);
    }
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("Download failed with status {}: {url}", response.status());
    }

    if let Some(t) = tracker {
        match resolve_total_size(client, &response, url).await {
            Some(len) => {
                info!("Resolved size for {name}: {len} bytes");
                // `start_file` already zeroed the per-file counters; a plain
                // store of the resolved total is what we want. Broadcast at
                // once so the UI's MB display flips before the first chunk.
                t.total_bytes.store(len, Ordering::Relaxed);
                t.broadcast_progress();
            }
            None => info!(
                "No size header for {name} (X-Linked-Size + Content-Length absent on GET and HEAD); progress will only update at file boundaries"
            ),
        }
    }

    let tmp = parent.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    let mut file = fs::File::create(&tmp).await?;
    // No byte cap for model files (unlike the registry install path); the
    // download is cancellable and progress is reported through `tracker`.
    let result = stream_body_to_writer(
        response,
        &mut file,
        None,
        || tracker.is_some_and(|t| t.is_cancelled()),
        |n| {
            if let Some(t) = tracker {
                t.bytes_downloaded.fetch_add(n, Ordering::Relaxed);
                // The tracker throttles to 1% increments, so per-chunk is fine.
                t.broadcast_progress();
            }
        },
    )
    .await;
    let actual = match result {
        Ok((_, actual)) => actual,
        Err(StreamError::Cancelled) => {
            let _ = fs::remove_file(&tmp).await;
            anyhow::bail!("download cancelled");
        }
        Err(e) => return Err(e.into()),
    };
    // Flush already happened in the helper; fsync before publishing at the final
    // path so a crash can't leave a renamed-but-unflushed file.
    file.sync_all().await?;
    drop(file);

    // Verify before publishing the file at its final path. The hash is always
    // computed; verify only when the item declares a pin.
    if let Some(expected) = item.sha256.as_ref()
        && !sha256_matches(&actual, expected)
    {
        let _ = fs::remove_file(&tmp).await;
        anyhow::bail!("SHA-256 mismatch for {name}: expected {expected}, got {actual}");
    }

    fs::rename(&tmp, dest).await?;
    Ok(())
}

/// Download a model's files into the backend directory.
///
/// Each item carries its own URL and absolute `destination`; parent
/// directories are created as needed. A file already present (non-zero size,
/// and matching its declared `sha256`) is skipped; otherwise it is downloaded
/// and, when a `sha256` is declared, verified.
///
/// When `tracker` is `Some`, per-file and per-byte progress is reported through
/// it. When `None`, downloads run silently (used by unit tests and one-off
/// calls that don't go through the daemon's `DownloadStateManager`).
///
/// `starting_file_index` lets the caller compose multiple `download_files`
/// calls against a single tracker so the file counter stays monotonic — pass
/// `0` for the first call and the running total for subsequent ones.
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
    // The provider is installed once in `main` before any download runs, so no
    // redundant install here (Tier 2 #8).
    let client = super_stt_forge::http::download_client();

    for (offset, item) in items.iter().enumerate() {
        if let Some(t) = tracker
            && t.is_cancelled()
        {
            anyhow::bail!("download cancelled");
        }
        download_one(&client, item, tracker, starting_file_index + offset).await?;
    }

    Ok(())
}
