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
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::UNIX_EPOCH;
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

/// Sidecar recording that a file was verified against its pinned hash, so a
/// later load can accept it without streaming it through SHA-256 again.
///
/// Without this, every model load re-hashes every pinned file it finds on
/// disk: a several-hundred-megabyte weight file is read and digested in full
/// on each daemon start, to reach the same answer as last time.
///
/// The stamp only skips work already done — it never widens what is trusted
/// off the network, where the pin is still checked before the file is
/// published at its final path. Anything that could mean different bytes —
/// a change in size, a change in mtime, or a different pin in the manifest —
/// falls back to the full digest. It is not a defence against local
/// tampering: whoever can rewrite the model file in place can rewrite its
/// stamp too, and already holds the directory either way.
#[derive(Serialize, Deserialize)]
struct VerifiedStamp {
    /// The pinned hash the file was verified against, hex-encoded.
    sha256: String,
    /// The file's size, in bytes, when it passed.
    size: u64,
    /// The file's mtime, in nanoseconds since the Unix epoch, when it passed.
    mtime_ns: u64,
}

/// Where `dest`'s stamp lives: a dotfile beside it, so it is carried and
/// removed with the model directory rather than kept in a separate cache that
/// could outlive the file it describes.
fn stamp_path(dest: &Path) -> PathBuf {
    let mut name = OsString::from(".");
    match dest.file_name() {
        Some(base) => name.push(base),
        None => name.push("file"),
    }
    name.push(".verified");
    dest.with_file_name(name)
}

/// Nanoseconds since the Unix epoch. `None` when the platform has no mtime for
/// the file or it predates the epoch, which simply leaves the file to be
/// hashed as before.
fn mtime_ns(md: &std::fs::Metadata) -> Option<u64> {
    u64::try_from(
        md.modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos(),
    )
    .ok()
}

/// Whether `dest` carries a stamp saying these exact bytes already passed
/// `expected`.
async fn stamp_says_verified(dest: &Path, md: &std::fs::Metadata, expected: &str) -> bool {
    let Ok(raw) = fs::read(stamp_path(dest)).await else {
        return false;
    };
    let Ok(stamp) = serde_json::from_slice::<VerifiedStamp>(&raw) else {
        return false;
    };
    stamp.size == md.len()
        && Some(stamp.mtime_ns) == mtime_ns(md)
        && sha256_matches(&stamp.sha256, expected)
}

/// Record that `dest` passed `expected`, best effort: a stamp that cannot be
/// written costs a re-hash on the next load and nothing else.
async fn write_stamp(dest: &Path, md: &std::fs::Metadata, expected: &str) {
    let Some(mtime_ns) = mtime_ns(md) else {
        return;
    };
    let stamp = VerifiedStamp {
        sha256: expected.to_string(),
        size: md.len(),
        mtime_ns,
    };
    if let Ok(json) = serde_json::to_vec(&stamp) {
        let _ = fs::write(stamp_path(dest), json).await;
    }
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
    let Some(expected) = sha256 else {
        return Ok(Some(md.len()));
    };
    // A stamp left by an earlier verification of these exact bytes answers the
    // question the digest would, so a pinned multi-gigabyte file is not read
    // end to end on every load.
    if stamp_says_verified(dest, &md, expected).await {
        return Ok(Some(md.len()));
    }
    let actual = sha256_hex_of_file(dest).await?;
    if !sha256_matches(&actual, expected) {
        info!(
            "Hash mismatch for existing {} (expected {expected}, got {actual}); re-downloading",
            dest.display()
        );
        return Ok(None);
    }
    write_stamp(dest, &md, expected).await;
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
        if let Some(t) = tracker {
            info!(
                "File {}/{} already present, skipping download: {}",
                file_index + 1,
                t.total_files.load(Ordering::Relaxed),
                dest.display()
            );
            t.start_file(&name, file_index);
            t.bytes_downloaded.store(len, Ordering::Relaxed);
            t.total_bytes.store(len, Ordering::Relaxed);
            t.broadcast_progress();
        } else {
            info!("Already present: {}", dest.display());
        }
        return Ok(());
    }

    let url = &item.url;
    if let Some(t) = tracker {
        info!(
            "Downloading file {}/{}: {url} -> {}",
            file_index + 1,
            t.total_files.load(Ordering::Relaxed),
            dest.display()
        );
        t.start_file(&name, file_index);
    } else {
        info!("Downloading {url} -> {}", dest.display());
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

    // These bytes were just verified against the pin; stamp them so the next
    // load doesn't repeat the digest.
    if let Some(expected) = item.sha256.as_ref()
        && let Ok(md) = fs::metadata(dest).await
    {
        write_stamp(dest, &md, expected).await;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Two payloads of the same length, so a test can swap one for the other
    /// and leave every cheap signal about the file unchanged.
    const ORIGINAL: &[u8] = b"the original bytes";
    const SWAPPED: &[u8] = b"the swapped bytes!";

    /// Overwrite `path`, restoring the mtime it had, so only a digest could
    /// tell the content changed.
    fn rewrite_preserving_mtime(path: &Path, bytes: &[u8]) {
        let mtime = std::fs::metadata(path)
            .and_then(|md| md.modified())
            .unwrap();
        std::fs::write(path, bytes).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(mtime))
            .unwrap();
    }

    /// Write `ORIGINAL` into a fresh directory and return it with its own hash
    /// as the pin.
    async fn pinned_file() -> (tempfile::TempDir, PathBuf, String) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("weights.bin");
        std::fs::write(&file, ORIGINAL).unwrap();
        let pin = sha256_hex_of_file(&file).await.unwrap();
        (dir, file, pin)
    }

    /// The first verification hashes the file and leaves a stamp; the second
    /// takes the stamp's word for it. Proven by swapping the content for
    /// same-length bytes with the same mtime — a re-hash would reject them.
    #[tokio::test]
    async fn a_stamp_stands_in_for_the_rehash() {
        let (_dir, file, pin) = pinned_file().await;

        let size = ORIGINAL.len() as u64;
        assert_eq!(
            usable_existing(&file, Some(&pin)).await.unwrap(),
            Some(size)
        );
        assert!(stamp_path(&file).exists());

        rewrite_preserving_mtime(&file, SWAPPED);
        assert_eq!(
            usable_existing(&file, Some(&pin)).await.unwrap(),
            Some(size)
        );
    }

    /// A file whose size no longer matches its stamp is hashed again, and the
    /// mismatch sends it back for re-download.
    #[tokio::test]
    async fn a_changed_file_is_rehashed_despite_its_stamp() {
        let (_dir, file, pin) = pinned_file().await;
        assert!(usable_existing(&file, Some(&pin)).await.unwrap().is_some());

        std::fs::write(&file, b"truncated").unwrap();
        assert_eq!(usable_existing(&file, Some(&pin)).await.unwrap(), None);
    }

    /// A stamp is scoped to the pin it was taken against: when the manifest
    /// moves to a new revision, the old stamp doesn't vouch for it.
    #[tokio::test]
    async fn a_changed_pin_is_rehashed_despite_its_stamp() {
        let (_dir, file, pin) = pinned_file().await;
        assert!(usable_existing(&file, Some(&pin)).await.unwrap().is_some());

        let moved_revision = "0".repeat(64);
        assert_eq!(
            usable_existing(&file, Some(&moved_revision)).await.unwrap(),
            None
        );
    }

    /// Nothing is hashed or stamped for a file the manifest doesn't pin.
    #[tokio::test]
    async fn an_unpinned_file_is_accepted_without_a_stamp() {
        let (_dir, file, _pin) = pinned_file().await;

        assert!(usable_existing(&file, None).await.unwrap().is_some());
        assert!(!stamp_path(&file).exists());
    }

    /// An empty file is never usable, pinned or not.
    #[tokio::test]
    async fn an_empty_file_is_not_usable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("weights.bin");
        std::fs::write(&file, b"").unwrap();

        assert_eq!(usable_existing(&file, None).await.unwrap(), None);
    }
}
