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
///
/// Reports through `tracker` as it goes: a file without a usable stamp is read
/// end to end, which for multi-GB weights is seconds of real work, and without
/// per-chunk updates the client's card sits frozen on the *previous* file's
/// numbers for the whole of it. Each chunk also honours the cancel flag, so
/// Cancel is live during verification the same way it is during a download.
async fn sha256_hex_of_file(
    path: &Path,
    tracker: Option<&Arc<DownloadProgressTracker>>,
) -> Result<String> {
    let mut file = fs::File::open(path).await?;
    let mut ctx = Context::new(&SHA256);
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
        if let Some(t) = tracker {
            if t.is_cancelled() {
                anyhow::bail!("download cancelled");
            }
            t.bytes_downloaded
                .fetch_add(u64::try_from(n).unwrap_or(u64::MAX), Ordering::Relaxed);
            // The tracker throttles to 1% increments, so per-chunk is fine.
            t.broadcast_progress();
        }
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
async fn usable_existing(
    dest: &Path,
    sha256: Option<&str>,
    tracker: Option<&Arc<DownloadProgressTracker>>,
) -> Result<Option<u64>> {
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
    // end to end on every load. Nothing to report when it hits: the caller
    // publishes this file at 100% the moment we return.
    if stamp_says_verified(dest, &md, expected).await {
        return Ok(Some(md.len()));
    }
    if let Some(t) = tracker {
        // Size first, bytes after: the hash loop fills `bytes_downloaded` from
        // zero, so publishing the size here is what gives the bar a denominator
        // before the first chunk is read.
        t.total_bytes.store(md.len(), Ordering::Relaxed);
        t.broadcast_progress();
    }
    let actual = sha256_hex_of_file(dest, tracker).await?;
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

    // Point the tracker at this file *before* deciding what to do with it. The
    // check itself can be the slow part — a file whose stamp is missing is
    // hashed end to end — so a tracker still describing the previous file
    // leaves the client frozen on stale numbers for the whole of it. No
    // broadcast yet: the next publish belongs to whichever phase this file
    // turns out to be in, so a file that isn't on disk goes straight to
    // "downloading" without a verify flicker.
    if let Some(t) = tracker {
        t.start_file(&name, file_index);
        t.mark_verifying();
    }

    // Skip a file that's already on disk (verified against `sha256` when set).
    // Per-file counters: both totals are this file's size so the UI shows
    // "X.X / X.X MB" at 100%, then the file_index advances next iteration.
    if let Some(len) = usable_existing(dest, item.sha256.as_deref(), tracker).await? {
        if let Some(t) = tracker {
            info!(
                "File {}/{} already present, skipping download: {}",
                file_index + 1,
                t.total_files.load(Ordering::Relaxed),
                dest.display()
            );
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
        // Bytes are about to come off the network, so the phase changes and
        // this file's accounting starts over — a failed verification leaves
        // `bytes_downloaded` holding however much of the bad copy was hashed.
        t.bytes_downloaded.store(0, Ordering::Relaxed);
        t.total_bytes.store(0, Ordering::Relaxed);
        t.mark_downloading();
        t.broadcast_progress();
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
                // The counters were zeroed on entering the download phase
                // above; a plain store of the resolved total is what we want.
                // Broadcast at once so the UI's MB display flips before the
                // first chunk.
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
/// it, in two phases per file: `verifying` while an existing copy is checked
/// (a file whose stamp is missing is hashed end to end, and that is reported
/// byte-by-byte like any other work), then `downloading` only if that check
/// came up short. A fully cached model therefore never reports a download —
/// which is the difference between a client saying "checking files" and a
/// client showing a download bar for files it already has. When `None`,
/// downloads run silently (used by unit tests and one-off calls that don't go
/// through the daemon's `DownloadStateManager`).
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
    let client =
        super_engine_forge::http::download_client(super_stt_registry_types::Stt::USER_AGENT);

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
        let pin = sha256_hex_of_file(&file, None).await.unwrap();
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
            usable_existing(&file, Some(&pin), None).await.unwrap(),
            Some(size)
        );
        assert!(stamp_path(&file).exists());

        rewrite_preserving_mtime(&file, SWAPPED);
        assert_eq!(
            usable_existing(&file, Some(&pin), None).await.unwrap(),
            Some(size)
        );
    }

    /// A file whose size no longer matches its stamp is hashed again, and the
    /// mismatch sends it back for re-download.
    #[tokio::test]
    async fn a_changed_file_is_rehashed_despite_its_stamp() {
        let (_dir, file, pin) = pinned_file().await;
        assert!(
            usable_existing(&file, Some(&pin), None)
                .await
                .unwrap()
                .is_some()
        );

        std::fs::write(&file, b"truncated").unwrap();
        assert_eq!(
            usable_existing(&file, Some(&pin), None).await.unwrap(),
            None
        );
    }

    /// A stamp is scoped to the pin it was taken against: when the manifest
    /// moves to a new revision, the old stamp doesn't vouch for it.
    #[tokio::test]
    async fn a_changed_pin_is_rehashed_despite_its_stamp() {
        let (_dir, file, pin) = pinned_file().await;
        assert!(
            usable_existing(&file, Some(&pin), None)
                .await
                .unwrap()
                .is_some()
        );

        let moved_revision = "0".repeat(64);
        assert_eq!(
            usable_existing(&file, Some(&moved_revision), None)
                .await
                .unwrap(),
            None
        );
    }

    /// Nothing is hashed or stamped for a file the manifest doesn't pin.
    #[tokio::test]
    async fn an_unpinned_file_is_accepted_without_a_stamp() {
        let (_dir, file, _pin) = pinned_file().await;

        assert!(usable_existing(&file, None, None).await.unwrap().is_some());
        assert!(!stamp_path(&file).exists());
    }

    /// An empty file is never usable, pinned or not.
    #[tokio::test]
    async fn an_empty_file_is_not_usable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("weights.bin");
        std::fs::write(&file, b"").unwrap();

        assert_eq!(usable_existing(&file, None, None).await.unwrap(), None);
    }

    // --- The phases a load reports ------------------------------------------

    /// `download_files` builds a client up front even when every file turns
    /// out to be cached, and `reqwest` panics without a crypto provider — the
    /// daemon installs one in `main`, so a test standing in for it does too.
    /// Idempotent, so every test can call it.
    fn install_crypto_provider() {
        super_engine_forge::http::install_crypto_provider();
    }

    /// Write `contents` at `dir/name` and return the item that declares it,
    /// pinned to the hash of what was written — i.e. a file already
    /// provisioned.
    async fn cached_item(dir: &Path, name: &str, contents: &[u8]) -> DownloadItem {
        let destination = dir.join(name);
        fs::write(&destination, contents)
            .await
            .expect("write fixture");
        let sha256 = sha256_hex_of_file(&destination, None)
            .await
            .expect("hash fixture");
        DownloadItem {
            // Unreachable on purpose: these tests are about the paths that must
            // never touch the network. One that did would fail here rather than
            // pass quietly.
            url: "http://127.0.0.1:1/never-fetched".to_string(),
            destination,
            sha256: Some(sha256),
        }
    }

    fn tracker_on(
        bus: &Arc<crate::daemon::events::EventBus>,
        total_files: usize,
    ) -> Arc<DownloadProgressTracker> {
        Arc::new(
            DownloadProgressTracker::new(
                "test-model".to_string(),
                "github.com/super-stt/test".to_string(),
                super_stt_shared::models::protocol::TRANSCRIPTION_STAGE,
                total_files,
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .with_event_bus(Arc::clone(bus)),
        )
    }

    /// Every `status` published to the bus, in order, until it goes quiet.
    async fn published_statuses(rx: &mut crate::daemon::events::AnyReceiver) -> Vec<String> {
        let mut statuses = Vec::new();
        while let Ok(Ok((_, payload))) =
            tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv_json()).await
        {
            if let Some(status) = payload["status"].as_str() {
                statuses.push(status.to_string());
            }
        }
        statuses
    }

    /// The regression this phase split exists for: loading a model whose files
    /// are all on disk must report `verifying` and nothing else. It used to
    /// report `downloading` for every cached file, so a load that fetched
    /// nothing showed a download bar — and sat on it for however long the
    /// checksums took.
    #[tokio::test]
    async fn a_fully_cached_model_verifies_and_never_reports_a_download() {
        install_crypto_provider();
        let dir = tempfile::tempdir().expect("temp dir");
        let items = vec![
            cached_item(dir.path(), "config.json", b"{}").await,
            cached_item(dir.path(), "model.safetensors", &vec![7u8; 4096]).await,
        ];

        let bus = Arc::new(crate::daemon::events::EventBus::new());
        let mut rx = bus.subscribe(crate::daemon::events::Topic::DownloadProgress);
        let tracker = tracker_on(&bus, items.len());

        download_files(&items, Some(&tracker), 0)
            .await
            .expect("files already on disk provision without a download");

        let statuses = published_statuses(&mut rx).await;
        assert!(
            !statuses.is_empty(),
            "a cached load still reports what it is doing"
        );
        assert!(
            statuses.iter().all(|s| s == "verifying"),
            "a cached load must never claim a download: {statuses:?}"
        );

        let progress = tracker.get_progress();
        assert_eq!(progress.file_index, 1, "the counter walked both files");
        assert_eq!(progress.bytes_downloaded, 4096);
        assert_eq!(progress.total_bytes, 4096);
    }

    /// Verification of a file with no usable stamp reports progress as it
    /// hashes. Without this, the card sits on the *previous* file's numbers for
    /// the whole checksum, which is what made a cached load look frozen.
    #[tokio::test]
    async fn verification_reports_progress_while_it_hashes() {
        install_crypto_provider();
        let dir = tempfile::tempdir().expect("temp dir");
        // Comfortably more than the 1 MiB hash chunk, so the loop publishes
        // more than once for this one file. No stamp exists yet, so this load
        // is the one that hashes it.
        let item = cached_item(dir.path(), "weights.bin", &vec![3u8; 5 * 1024 * 1024]).await;

        let bus = Arc::new(crate::daemon::events::EventBus::new());
        let mut rx = bus.subscribe(crate::daemon::events::Topic::DownloadProgress);
        let tracker = tracker_on(&bus, 1);

        download_files(std::slice::from_ref(&item), Some(&tracker), 0)
            .await
            .expect("cached file verifies");

        let mut percentages = Vec::new();
        while let Ok(Ok((_, payload))) =
            tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv_json()).await
        {
            percentages.push(payload["percentage"].as_f64().unwrap_or(-1.0));
        }
        assert!(
            percentages.len() > 2,
            "hashing a 5 MiB file should tick more than once: {percentages:?}"
        );
        assert!(
            percentages.windows(2).all(|w| w[0] <= w[1]),
            "verification progress must only go forward: {percentages:?}"
        );
        assert!(
            percentages.last().is_some_and(|p| (p - 100.0).abs() < 0.01),
            "verification ends with the file fully accounted for: {percentages:?}"
        );
    }

    /// A cached copy that no longer matches its pin is not a verified file: the
    /// phase flips to `downloading` and the bytes are fetched again. The
    /// counters start over with it, so the download's bar measures the download
    /// and not the bytes hashed on the way to rejecting the old copy.
    #[tokio::test]
    async fn a_stale_cached_file_flips_to_downloading_and_is_refetched() {
        install_crypto_provider();
        let dir = tempfile::tempdir().expect("temp dir");
        let good = vec![9u8; 2048];
        let mut item = cached_item(dir.path(), "weights.bin", &good).await;
        // Same name, wrong bytes: what a truncated or tampered cache looks like.
        fs::write(&item.destination, vec![0u8; 2048])
            .await
            .expect("corrupt the cached copy");

        let mut server = mockito::Server::new_async().await;
        let asset = server
            .mock("GET", "/weights.bin")
            .with_status(200)
            .with_body(good.clone())
            .create_async()
            .await;
        item.url = format!("{}/weights.bin", server.url());

        let bus = Arc::new(crate::daemon::events::EventBus::new());
        let mut rx = bus.subscribe(crate::daemon::events::Topic::DownloadProgress);
        let tracker = tracker_on(&bus, 1);

        download_files(std::slice::from_ref(&item), Some(&tracker), 0)
            .await
            .expect("a stale file is replaced, not fatal");

        asset.assert_async().await;
        assert_eq!(
            fs::read(&item.destination)
                .await
                .expect("read repaired file"),
            good,
            "the stale copy is replaced by the real one"
        );

        let statuses = published_statuses(&mut rx).await;
        assert_eq!(
            statuses.first().map(String::as_str),
            Some("verifying"),
            "the check comes first: {statuses:?}"
        );
        assert!(
            statuses.iter().any(|s| s == "downloading"),
            "rejecting the cached copy must announce the download: {statuses:?}"
        );
    }

    /// Cancel is live during verification. Hashing an unstamped multi-GB file
    /// is seconds of work, and a Cancel that only took effect once the download
    /// started would do nothing at all for a fully cached load.
    #[tokio::test]
    async fn cancelling_during_verification_stops_the_hash() {
        let dir = tempfile::tempdir().expect("temp dir");
        let item = cached_item(dir.path(), "weights.bin", &vec![1u8; 4096]).await;

        let tracker = Arc::new(DownloadProgressTracker::new(
            "test-model".to_string(),
            "github.com/super-stt/test".to_string(),
            super_stt_shared::models::protocol::TRANSCRIPTION_STAGE,
            1,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
        ));

        let err = usable_existing(&item.destination, item.sha256.as_deref(), Some(&tracker))
            .await
            .expect_err("a cancelled verification does not report a usable file");
        assert!(
            err.to_string().contains("cancelled"),
            "cancellation surfaces as itself, not as a hash mismatch: {err}"
        );
    }
}
