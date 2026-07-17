// SPDX-License-Identifier: GPL-3.0-only
//! Install pipeline. State machine: Resolving → Downloading → Verifying →
//! Extracting → Installing → Rescanning → Done | Failed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use ring::digest::SHA256;
use super_stt_shared::registry::events::{InstallError, InstallPhase};
use thiserror::Error;
use tokio::fs;

use crate::download_stream::{StreamError, stream_body_to_writer};
use crate::registry::compat::Selection;
use crate::registry::index_schema::{IndexAsset, IndexBackend};
use super_stt_registry_types::verify::{
    MAX_MANIFEST_BYTES, sha256_matches, tar_budget_step, tar_entry_unsafe_reason, unpack_cap,
};

/// Absolute ceiling on a single downloaded asset when its size is not declared
/// in the index. Declared sizes are honored directly (plus a small margin).
const MAX_DOWNLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Slack allowed over a declared asset size before a download is rejected.
const DOWNLOAD_SIZE_MARGIN: u64 = 1024 * 1024;
/// The byte ceiling for a download given the index-declared `expected_size`
/// (0 when unknown).
fn download_cap(expected_size: u64) -> u64 {
    if expected_size == 0 {
        MAX_DOWNLOAD_BYTES
    } else {
        expected_size.saturating_add(DOWNLOAD_SIZE_MARGIN)
    }
}

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("hash mismatch: expected `{expected}`, got `{actual}`")]
    HashMismatch { expected: String, actual: String },
    #[error("tarball contains unsafe entry: {0}")]
    TarUnsafe(String),
    #[error("download exceeds {limit} bytes")]
    TooLarge { limit: u64 },
}

impl PipelineError {
    #[must_use]
    pub fn as_typed(&self, phase: InstallPhase) -> (InstallPhase, InstallError) {
        match self {
            PipelineError::Network(_) => (phase, InstallError::DownloadFailed),
            PipelineError::HashMismatch { .. } => {
                (InstallPhase::Verifying, InstallError::AssetHashMismatch)
            }
            PipelineError::TarUnsafe(_) => (InstallPhase::Extracting, InstallError::TarballUnsafe),
            PipelineError::TooLarge { .. } => {
                (InstallPhase::Downloading, InstallError::DownloadFailed)
            }
            PipelineError::Io(_) => (phase, InstallError::InstallIoError),
        }
    }
}

pub struct Pipeline<F> {
    pub backends_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub http: reqwest::Client,
    pub on_progress: Arc<F>,
}

/// Run an install. `entry` and `selection` come from the registry client +
/// compat module. Returns the installed version on success.
///
/// # Errors
/// Returns `(InstallPhase, InstallError)` on failure, including an
/// `Incompatible` error if `selection` does not match an asset on `entry`.
pub async fn run<F>(
    p: &Pipeline<F>,
    entry: &IndexBackend,
    selection: &Selection,
) -> Result<String, (InstallPhase, InstallError)>
where
    F: Fn(InstallPhase, Option<(u64, Option<u64>)>) + Send + Sync,
{
    use InstallPhase as P;

    (p.on_progress)(P::Resolving, None);

    let kind_subdir = match selection {
        Selection::Wasm => false,
        Selection::Subprocess { .. } => true,
        Selection::Incompatible { reason: _ } => {
            return Err((P::Resolving, InstallError::Incompatible));
        }
    };

    let partial_name = format!(
        "{}-{}.{}.partial",
        entry.id,
        entry.version,
        if kind_subdir { "tar.gz" } else { "wasm" }
    );
    let partial_path = p.cache_dir.join(&partial_name);
    fs::create_dir_all(&p.cache_dir)
        .await
        .map_err(|e| PipelineError::Io(e).as_typed(P::Downloading))?;

    // Fetch the verified artifact into `partial_path`: a single download for a
    // wasm or single-file subprocess asset, or a per-part download +
    // concatenation for a multi-part subprocess archive.
    download_and_verify(p, entry, selection, &partial_path).await?;

    (p.on_progress)(P::Extracting, None);
    let staging = p
        .backends_dir
        .join(".staging")
        .join(format!("{}-{}", entry.id, entry.version));
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging).await;
    }
    fs::create_dir_all(&staging)
        .await
        .map_err(|e| PipelineError::Io(e).as_typed(P::Extracting))?;
    if kind_subdir {
        extract_tarball(&partial_path, &staging).map_err(|e| e.as_typed(P::Extracting))?;
    } else {
        let dest = staging.join(&entry.entrypoint);
        fs::copy(&partial_path, &dest)
            .await
            .map_err(|e| PipelineError::Io(e).as_typed(P::Extracting))?;
    }

    // Install the backend's own manifest — the exact bytes pinned in the index,
    // verified here and written verbatim. Every installable entry carries a
    // pinned manifest asset; an entry without one is not installable. We never
    // trust whatever may have been packed inside the tarball.
    let pin = entry
        .manifest
        .as_ref()
        .ok_or((P::Installing, InstallError::ManifestInvalid))?;
    let toml_text = fetch_verified_manifest(&p.http, entry, pin).await?;
    fs::write(staging.join("backend.toml"), toml_text)
        .await
        .map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;

    (p.on_progress)(P::Installing, None);
    let final_path = p.backends_dir.join(&entry.id);
    swap_into_place(&staging, &final_path)
        .await
        .map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;
    let _ = fs::remove_file(&partial_path).await;

    (p.on_progress)(P::Rescanning, None);
    Ok(entry.version.clone())
}

/// Download the artifact `selection` names into `partial_path` and verify its
/// integrity: a single hashed download for a wasm or single-file subprocess
/// asset, or a per-part download + concatenation for a multi-part archive.
async fn download_and_verify<F>(
    p: &Pipeline<F>,
    entry: &IndexBackend,
    selection: &Selection,
    partial_path: &Path,
) -> Result<(), (InstallPhase, InstallError)>
where
    F: Fn(InstallPhase, Option<(u64, Option<u64>)>) + Send + Sync,
{
    use InstallPhase as P;
    (p.on_progress)(P::Downloading, Some((0, None)));
    match selection {
        Selection::Wasm => {
            let a = entry
                .assets
                .wasm
                .as_ref()
                .ok_or((P::Resolving, InstallError::Incompatible))?;
            let actual = stream_download(&p.http, &a.url, a.size, partial_path, &p.on_progress)
                .await
                .map_err(|e| e.as_typed(P::Downloading))?;
            (p.on_progress)(P::Verifying, None);
            verify_asset_sha(&actual, &a.sha256, &entry.source, partial_path).await
        }
        Selection::Subprocess { index } => {
            let a = entry
                .assets
                .subprocess
                .get(*index)
                .ok_or((P::Resolving, InstallError::Incompatible))?;
            if a.parts.is_empty() {
                let url = a
                    .url
                    .as_deref()
                    .ok_or((P::Resolving, InstallError::Incompatible))?;
                let actual = stream_download(
                    &p.http,
                    url,
                    a.size.unwrap_or(0),
                    partial_path,
                    &p.on_progress,
                )
                .await
                .map_err(|e| e.as_typed(P::Downloading))?;
                (p.on_progress)(P::Verifying, None);
                verify_asset_sha(
                    &actual,
                    a.sha256.as_deref().unwrap_or(""),
                    &entry.source,
                    partial_path,
                )
                .await
            } else {
                // Multi-part: download each part, verify its SHA-256, and append
                // it to the partial `.tar.gz` (verification is inline, per part).
                download_verified_parts(&p.http, &a.parts, partial_path, &p.on_progress)
                    .await
                    .map_err(|e| e.as_typed(P::Downloading))?;
                (p.on_progress)(P::Verifying, None);
                Ok(())
            }
        }
        Selection::Incompatible { reason: _ } => Err((P::Resolving, InstallError::Incompatible)),
    }
}

/// Import-from-dir variant: the operator provided `src_dir`, so the
/// download/verify/extract phases don't apply. We still go through the
/// stage-then-rename dance so the install dir flips atomically, and we still
/// emit Resolving/Installing/Rescanning so the UI surface matches the
/// registry path.
///
/// Symlinks inside `src_dir` are rejected (matching the registry tarball
/// path), so an imported dir cannot smuggle in a link whose target's bytes
/// would be copied into the backend-readable install dir.
///
/// # Errors
/// Returns `(InstallPhase, InstallError)` on filesystem failure.
///
/// # Panics
/// None.
pub async fn run_local<F>(
    p: &Pipeline<F>,
    entry: &IndexBackend,
    src_dir: &Path,
) -> Result<String, (InstallPhase, InstallError)>
where
    F: Fn(InstallPhase, Option<(u64, Option<u64>)>) + Send + Sync,
{
    use InstallPhase as P;

    (p.on_progress)(P::Resolving, None);

    let staging = p
        .backends_dir
        .join(".staging")
        .join(format!("{}-{}", entry.id, entry.version));
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging).await;
    }
    fs::create_dir_all(&staging)
        .await
        .map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;

    let src = src_dir.to_path_buf();
    let dst = staging.clone();
    tokio::task::spawn_blocking(move || copy_dir_recursive(&src, &dst))
        .await
        .map_err(|e| {
            PipelineError::Io(std::io::Error::other(format!("copy join: {e}")))
                .as_typed(P::Installing)
        })?
        .map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;

    (p.on_progress)(P::Installing, None);
    let final_path = p.backends_dir.join(&entry.id);
    swap_into_place(&staging, &final_path)
        .await
        .map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;

    (p.on_progress)(P::Rescanning, None);
    Ok(entry.version.clone())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        // `read_dir`'s file_type does not follow links, so this detects the
        // link itself. Reject it: copying would follow the link and pull the
        // target's bytes (e.g. `creds -> ~/.ssh/id_rsa`) into the install dir.
        // Matches the tarball path's symlink policy.
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("refusing to import symlink entry: {}", from.display()),
            ));
        }
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Move `staging` into `final_path` as atomically as the filesystem allows:
/// move any existing dir aside to a `.old` sidecar, rename staging into place,
/// then delete the sidecar. A crash mid-swap leaves at most a recoverable
/// `<final>.old` rather than a missing backend; a rename failure rolls back.
async fn swap_into_place(staging: &Path, final_path: &Path) -> std::io::Result<()> {
    let mut sidecar_os = final_path.as_os_str().to_owned();
    sidecar_os.push(".old");
    let sidecar = PathBuf::from(sidecar_os);

    let moved_aside = if final_path.exists() {
        let _ = fs::remove_dir_all(&sidecar).await; // clear any stale sidecar
        fs::rename(final_path, &sidecar).await?;
        true
    } else {
        false
    };

    match fs::rename(staging, final_path).await {
        Ok(()) => {
            if moved_aside {
                let _ = fs::remove_dir_all(&sidecar).await;
            }
            Ok(())
        }
        Err(e) => {
            if moved_aside {
                let _ = fs::rename(&sidecar, final_path).await;
            }
            Err(e)
        }
    }
}

async fn stream_download<F>(
    http: &reqwest::Client,
    url: &str,
    expected_size: u64,
    dest: &Path,
    on_progress: &Arc<F>,
) -> Result<String, PipelineError>
where
    F: Fn(InstallPhase, Option<(u64, Option<u64>)>) + Send + Sync,
{
    let resp = http
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| {
            log::warn!(
                "install: download request failed ({url}){}: {e}",
                if e.is_timeout() { " [timeout]" } else { "" }
            );
            e
        })?;
    let total = resp.content_length();
    let cap = download_cap(expected_size);
    let mut file = tokio::fs::File::create(dest).await?;
    // Bound a server that streams past the index-declared size (or forever when
    // no size was declared) before it fills the disk.
    let mut bytes_done: u64 = 0;
    let result = stream_body_to_writer(
        resp,
        &mut file,
        Some(cap),
        || false,
        |n| {
            bytes_done += n;
            on_progress(InstallPhase::Downloading, Some((bytes_done, total)));
        },
    )
    .await;
    match result {
        Ok((_, sha)) => Ok(sha),
        Err(StreamError::TooLarge { limit }) => {
            let _ = tokio::fs::remove_file(dest).await;
            Err(PipelineError::TooLarge { limit })
        }
        Err(StreamError::Http(e)) => Err(PipelineError::Network(e)),
        Err(StreamError::Io(e)) => Err(PipelineError::Io(e)),
        // `should_cancel` is `|| false` here, so cancellation never occurs.
        Err(StreamError::Cancelled) => unreachable!("install download has no cancellation"),
    }
}

/// Verify a downloaded asset's SHA-256 against the index pin. An empty pin is
/// the custom-repo "unverified source" case (no index pre-computed a hash) —
/// TLS to the origin is then the only integrity guarantee.
async fn verify_asset_sha(
    actual: &str,
    expected: &str,
    source: &str,
    partial: &Path,
) -> Result<(), (InstallPhase, InstallError)> {
    if expected.is_empty() {
        log::warn!(
            "install({source}): no expected sha256; skipping asset verification (actual=`{actual}`)"
        );
        Ok(())
    } else if sha256_matches(actual, expected) {
        Ok(())
    } else {
        let _ = fs::remove_file(partial).await;
        Err((InstallPhase::Verifying, InstallError::AssetHashMismatch))
    }
}

/// Download each part in listed order, verifying its SHA-256, and append it to
/// `dest` — reconstituting the multi-part `.tar.gz` byte-for-byte. A bad part
/// removes the partial file and aborts. Progress spans the whole set.
async fn download_verified_parts<F>(
    http: &reqwest::Client,
    parts: &[IndexAsset],
    dest: &Path,
    on_progress: &Arc<F>,
) -> Result<(), PipelineError>
where
    F: Fn(InstallPhase, Option<(u64, Option<u64>)>) + Send + Sync,
{
    let total_expected: u64 = parts.iter().map(|p| p.size).sum();
    let mut out = tokio::fs::File::create(dest).await?;
    let mut overall: u64 = 0;
    for part in parts {
        let resp = http
            .get(&part.url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| {
                log::warn!(
                    "install: part request failed ({}){}: {e}",
                    part.url,
                    if e.is_timeout() { " [timeout]" } else { "" }
                );
                e
            })?;
        // Each part appends to the same `out`; progress spans the whole set.
        let cap = download_cap(part.size);
        let result = stream_body_to_writer(
            resp,
            &mut out,
            Some(cap),
            || false,
            |n| {
                overall += n;
                on_progress(
                    InstallPhase::Downloading,
                    Some((overall, Some(total_expected))),
                );
            },
        )
        .await;
        let actual = match result {
            Ok((_, actual)) => actual,
            Err(StreamError::TooLarge { limit }) => {
                drop(out);
                let _ = tokio::fs::remove_file(dest).await;
                return Err(PipelineError::TooLarge { limit });
            }
            Err(StreamError::Http(e)) => return Err(PipelineError::Network(e)),
            Err(StreamError::Io(e)) => return Err(PipelineError::Io(e)),
            Err(StreamError::Cancelled) => unreachable!("install download has no cancellation"),
        };
        if !part.sha256.is_empty() && !sha256_matches(&actual, &part.sha256) {
            drop(out);
            let _ = tokio::fs::remove_file(dest).await;
            return Err(PipelineError::HashMismatch {
                expected: part.sha256.clone(),
                actual,
            });
        }
    }
    Ok(())
}

fn extract_tarball(src: &Path, dest_dir: &Path) -> Result<(), PipelineError> {
    // Cap the uncompressed output relative to the (verified) compressed size so
    // a legitimate multi-GB bundle is allowed but a zip-bomb is not.
    let total_cap = unpack_cap(std::fs::metadata(src)?.len());
    // First pass: validate the archive (no unsafe entries) without unpacking.
    {
        let f = std::fs::File::open(src)?;
        let gz = flate2::read::GzDecoder::new(f);
        let mut archive = tar::Archive::new(gz);
        for entry in archive.entries()? {
            let entry = entry?;
            let path = entry.path()?;
            let s = path.to_string_lossy();
            if let Some(reason) =
                tar_entry_unsafe_reason(&s, entry.header().entry_type().is_symlink())
            {
                return Err(PipelineError::TarUnsafe(reason));
            }
        }
    }
    // Second pass: unpack with per-entry and total-output budgets so a small
    // archive cannot decompress into a disk-filling payload.
    let f = std::fs::File::open(src)?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(true);
    archive.set_overwrite(true);
    let mut total: u64 = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        total =
            tar_budget_step(entry.size(), total, total_cap).map_err(PipelineError::TarUnsafe)?;
        entry.unpack_in(dest_dir)?;
    }
    Ok(())
}

/// Download, verify, and validate the pinned `backend.toml` manifest asset,
/// returning the verified text to install verbatim.
///
/// The pinned hash already guarantees the bytes match what the indexer
/// validated; the remaining checks are defense in depth and enforce the
/// daemon's own invariants — the manifest must parse, pass runtime validation,
/// and declare a `source` and `entrypoint` consistent with the index entry, so
/// a backend cannot pin a manifest that claims another backend's identity or a
/// path that escapes its install dir.
async fn fetch_verified_manifest(
    http: &reqwest::Client,
    entry: &IndexBackend,
    pin: &IndexAsset,
) -> Result<String, (InstallPhase, InstallError)> {
    let bytes = download_manifest_bytes(http, &pin.url)
        .await
        .map_err(|e| e.as_typed(InstallPhase::Downloading))?;
    verify_manifest_bytes(&bytes, &pin.sha256, entry)
}

/// The pure verify step (no I/O): hash-check the bytes against the pin, then
/// parse, validate, and enforce identity/entrypoint consistency with the index
/// entry. Returns the verified manifest text.
fn verify_manifest_bytes(
    bytes: &[u8],
    pin_sha256: &str,
    entry: &IndexBackend,
) -> Result<String, (InstallPhase, InstallError)> {
    use crate::stt_models::backends::manifest::{Manifest, validate_runtime};
    use InstallPhase as P;

    // An empty pin sha is the Custom-repo "unverified source" case (no index
    // pre-computed a hash): TLS to the origin is the only integrity guarantee,
    // mirroring the binary-asset path. The validation below still runs.
    if pin_sha256.is_empty() {
        log::warn!(
            "install({}): no manifest sha256; skipping hash verification (unverified source)",
            entry.source
        );
    } else {
        let actual = hex::encode(ring::digest::digest(&SHA256, bytes).as_ref());
        if !sha256_matches(&actual, pin_sha256) {
            return Err((P::Verifying, InstallError::AssetHashMismatch));
        }
    }

    let reject = |reason: &str| {
        log::warn!(
            "install({}): pinned manifest rejected: {reason}",
            entry.source
        );
        (P::Installing, InstallError::ManifestInvalid)
    };
    let text = std::str::from_utf8(bytes)
        .map_err(|_| reject("not valid UTF-8"))?
        .to_string();
    // `Manifest::parse` (not raw `toml::from_str`) so install-time verification
    // shares the canonical parser's safety guards — entrypoint + per-file
    // `destination` traversal rejection and the `file`-xor-`parts` / empty-`file`
    // normalization — instead of hand-reimplementing only a subset here. A future
    // guard added to `parse` then applies here automatically (audit 2 Tier 2 #11).
    let m = Manifest::parse(&text).map_err(|e| reject(&format!("parse: {e}")))?;
    validate_runtime(&m).map_err(|e| reject(&format!("validation: {e}")))?;
    if m.backend.source != entry.source {
        return Err(reject(&format!(
            "manifest source `{}` != index source `{}`",
            m.backend.source, entry.source
        )));
    }
    // Index-consistency check only — the entrypoint's path safety is already
    // enforced by `Manifest::parse` above.
    if m.backend.entrypoint != entry.entrypoint {
        return Err(reject("entrypoint inconsistent with index"));
    }
    Ok(text)
}

/// Stream the manifest asset into memory, capped at [`MAX_MANIFEST_BYTES`].
async fn download_manifest_bytes(
    http: &reqwest::Client,
    url: &str,
) -> Result<Vec<u8>, PipelineError> {
    let resp = http.get(url).send().await?.error_for_status()?;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if buf.len() as u64 + chunk.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(PipelineError::TooLarge {
                limit: MAX_MANIFEST_BYTES,
            });
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::index_schema::*;

    const PINNED_MANIFEST: &str = r#"
[backend]
source = "github.com/x/y"
name = "X"
version = "1.0.0"
kind = "subprocess"
entrypoint = "x"
contract = "v1"
description = "Test backend."

[[models]]
name = "m"
provider = "local_x"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]
"#;

    fn sha_hex(bytes: &[u8]) -> String {
        hex::encode(ring::digest::digest(&SHA256, bytes).as_ref())
    }

    #[test]
    fn pinned_manifest_verifies_and_installs_verbatim() {
        let entry = minimal_entry(); // source github.com/x/y, entrypoint "x"
        let bytes = PINNED_MANIFEST.as_bytes();
        let out = verify_manifest_bytes(bytes, &sha_hex(bytes), &entry).expect("valid manifest");
        // The exact bytes are installed — no re-encoding.
        assert_eq!(out, PINNED_MANIFEST);
    }

    #[test]
    fn pinned_manifest_hash_mismatch_is_rejected() {
        let entry = minimal_entry();
        let err =
            verify_manifest_bytes(PINNED_MANIFEST.as_bytes(), "deadbeef", &entry).unwrap_err();
        assert_eq!(err.1, InstallError::AssetHashMismatch);
    }

    #[test]
    fn pinned_manifest_identity_spoof_is_rejected() {
        let entry = minimal_entry(); // index says source github.com/x/y
        // A correctly-hashed manifest that claims a different identity must not
        // be installed under this entry's id.
        let spoofed = PINNED_MANIFEST.replace("github.com/x/y", "github.com/evil/z");
        let bytes = spoofed.as_bytes();
        let err = verify_manifest_bytes(bytes, &sha_hex(bytes), &entry).unwrap_err();
        assert_eq!(err.1, InstallError::ManifestInvalid);
    }

    #[test]
    fn pinned_manifest_unparseable_is_rejected() {
        let entry = minimal_entry();
        let bytes = b"this is not toml = = =";
        let err = verify_manifest_bytes(bytes, &sha_hex(bytes), &entry).unwrap_err();
        assert_eq!(err.1, InstallError::ManifestInvalid);
    }

    fn minimal_entry() -> IndexBackend {
        IndexBackend {
            id: "x".into(),
            source: "github.com/x/y".into(),
            version: "1.0.0".into(),
            tag: "v1.0.0".into(),
            name: "X".into(),
            description: None,
            license: String::new(),
            kind: "subprocess".into(),
            contract: "v1".into(),
            entrypoint: "x".into(),
            allowed_hosts: vec![],
            online: false,
            supports_gpu: false,
            supports_cpu: true,
            models: vec![],
            secrets: vec![],
            options: vec![],
            assets: IndexAssets::default(),
            index_stale: None,
            manifest: None,
        }
    }

    #[tokio::test]
    async fn run_rejects_unmatched_selection_without_panicking() {
        let entry = minimal_entry();
        let pipeline = Pipeline {
            backends_dir: std::env::temp_dir(),
            cache_dir: std::env::temp_dir(),
            http: reqwest::Client::new(),
            on_progress: Arc::new(|_, _| {}),
        };
        // Subprocess index out of range (empty assets) — must be Incompatible, not a panic.
        let err = run(&pipeline, &entry, &Selection::Subprocess { index: 0 })
            .await
            .unwrap_err();
        assert!(matches!(err.1, InstallError::Incompatible));
        // Wasm selection but the entry has no wasm asset.
        let err = run(&pipeline, &entry, &Selection::Wasm).await.unwrap_err();
        assert!(matches!(err.1, InstallError::Incompatible));
    }

    #[test]
    fn download_cap_uses_declared_size_or_absolute_max() {
        assert_eq!(download_cap(0), MAX_DOWNLOAD_BYTES);
        assert_eq!(download_cap(1000), 1000 + DOWNLOAD_SIZE_MARGIN);
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_recursive_rejects_symlinks() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("real.txt"), b"hi").unwrap();
        std::os::unix::fs::symlink("/etc/hostname", src.path().join("link")).unwrap();
        let dst = tempfile::tempdir().unwrap();
        let err = copy_dir_recursive(src.path(), &dst.path().join("out")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn swap_into_place_replaces_existing_and_clears_sidecar() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let final_path = root.path().join("backend");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("new.txt"), b"new").unwrap();
        std::fs::create_dir_all(&final_path).unwrap();
        std::fs::write(final_path.join("old.txt"), b"old").unwrap();

        swap_into_place(&staging, &final_path).await.unwrap();

        assert!(final_path.join("new.txt").exists());
        assert!(!final_path.join("old.txt").exists());
        let mut sidecar = final_path.as_os_str().to_owned();
        sidecar.push(".old");
        assert!(!Path::new(&sidecar).exists(), "sidecar must be cleaned up");
    }

    /// The multi-part path downloads each part in order, verifies its SHA-256,
    /// and concatenates them byte-for-byte. Proves the multipart logic itself is
    /// sound, so a real-world failure points elsewhere (network/timeout).
    #[tokio::test]
    async fn multipart_download_concatenates_and_verifies_parts() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut server = mockito::Server::new_async().await;
        let p0 = vec![0xABu8; 4096];
        let p1 = vec![0xCDu8; 2048];
        let m0 = server
            .mock("GET", "/p0")
            .with_status(200)
            .with_body(p0.as_slice())
            .create_async()
            .await;
        let m1 = server
            .mock("GET", "/p1")
            .with_status(200)
            .with_body(p1.as_slice())
            .create_async()
            .await;
        let parts = vec![
            IndexAsset {
                url: format!("{}/p0", server.url()),
                size: p0.len() as u64,
                sha256: sha_hex(&p0),
            },
            IndexAsset {
                url: format!("{}/p1", server.url()),
                size: p1.len() as u64,
                sha256: sha_hex(&p1),
            },
        ];
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.tar.gz");
        let http = reqwest::Client::new();
        download_verified_parts(&http, &parts, &dest, &Arc::new(|_, _| {}))
            .await
            .expect("multipart download must succeed");
        m0.assert_async().await;
        m1.assert_async().await;
        let got = std::fs::read(&dest).unwrap();
        let mut want = p0.clone();
        want.extend_from_slice(&p1);
        assert_eq!(got, want, "reassembled bytes must be part0 || part1");
    }

    /// A download that can't finish within the client's request timeout surfaces
    /// as `PipelineError::Network`, which the install handler flattens to the
    /// user-visible `DownloadFailed`. This is the exact failure a multi-GB CUDA
    /// bundle hits when the client timeout is shorter than the transfer takes.
    #[tokio::test]
    async fn download_exceeding_client_timeout_is_a_download_failure() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        // A server that accepts the connection but never replies.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _hang = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                drop(stream);
            }
        });
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(300))
            .build()
            .unwrap();
        let parts = vec![IndexAsset {
            url: format!("http://{addr}/p0"),
            size: 1024,
            sha256: String::new(),
        }];
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.tar.gz");
        let err = download_verified_parts(&http, &parts, &dest, &Arc::new(|_, _| {}))
            .await
            .expect_err("a download that can't finish in time must fail");
        assert!(
            matches!(err, PipelineError::Network(_)),
            "a timeout must surface as a network error, got {err:?}"
        );
        assert_eq!(
            err.as_typed(InstallPhase::Downloading).1,
            InstallError::DownloadFailed,
            "network errors are the user-visible DownloadFailed"
        );
    }
}
