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
use crate::registry::compat::{self, Selection};
use crate::registry::index_schema::{IndexAsset, IndexBackend};
use super_stt_registry_types::verify::{
    MAX_MANIFEST_BYTES, sha256_matches, tar_budget_step, tar_entry_unsafe_reason, unpack_cap,
};

/// Absolute ceiling on a single downloaded asset when its size is not declared
/// in the index. Declared sizes are honored directly (plus a small margin).
const MAX_DOWNLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Slack allowed over a declared asset size before a download is rejected.
const DOWNLOAD_SIZE_MARGIN: u64 = 1024 * 1024;
/// The name of the shared staging root every install/update stages into
/// before the atomic swap: `<backends_dir>/.staging/<id>-<version>`. It is
/// a direct child of `backends_dir`, exactly like a backend's own install
/// directory, but it is never one itself — a directory scan over
/// `backends_dir` must always skip it explicitly rather than rely on its
/// manifest lookup happening to fail one level too shallow.
const STAGING_DIR_NAME: &str = ".staging";

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
        Selection::Incompatible { .. } | Selection::NeedsClientUpdate { .. } => {
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
    let staging = p.backends_dir.join(STAGING_DIR_NAME).join(format!(
        "{}-{}",
        crate::registry::install_dir_name(entry),
        entry.version
    ));
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

    // Record which variant this is. `backend.toml` lists them all, so without
    // this the daemon cannot tell a CUDA install from a CPU one after the fact.
    if let Some(selected) = compat::to_selected_asset(entry, selection) {
        crate::registry::installed::write(&staging, &selected)
            .map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;
    }

    (p.on_progress)(P::Installing, None);
    let final_path = p
        .backends_dir
        .join(crate::registry::install_dir_name(entry));
    // Before anything else touches the filesystem outside `staging`: the
    // swap below replaces whatever sits at `final_path`, and `preserve_models`
    // moves model files out of the directory currently serving this `source`.
    // Bailing here leaves both of those untouched.
    ensure_dir_free_for(&final_path, &entry.source)?;
    let inherit_from = previous_dir_for(&p.backends_dir, &entry.source)
        .await
        .unwrap_or_else(|| final_path.clone());
    preserve_models(&staging, &inherit_from)
        .await
        .map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;
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
        Selection::Incompatible { .. } | Selection::NeedsClientUpdate { .. } => {
            Err((P::Resolving, InstallError::Incompatible))
        }
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

    let staging = p.backends_dir.join(STAGING_DIR_NAME).join(format!(
        "{}-{}",
        crate::registry::install_dir_name(entry),
        entry.version
    ));
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging).await;
    }
    fs::create_dir_all(&staging)
        .await
        .map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;

    let src = src_dir.to_path_buf();
    let dst = staging.clone();
    let kind = entry.kind.clone();
    let entrypoint = entry.entrypoint.clone();
    tokio::task::spawn_blocking(move || copy_staged_backend(&src, &dst, &kind, &entrypoint))
        .await
        .map_err(|e| {
            PipelineError::Io(std::io::Error::other(format!("copy join: {e}")))
                .as_typed(P::Installing)
        })?
        .map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;

    (p.on_progress)(P::Installing, None);
    let final_path = p
        .backends_dir
        .join(crate::registry::install_dir_name(entry));
    // Before anything else touches the filesystem outside `staging`: the
    // swap below replaces whatever sits at `final_path`, and `preserve_models`
    // moves model files out of the directory currently serving this `source`.
    // Bailing here leaves both of those untouched.
    ensure_dir_free_for(&final_path, &entry.source)?;
    let inherit_from = previous_dir_for(&p.backends_dir, &entry.source)
        .await
        .unwrap_or_else(|| final_path.clone());
    preserve_models(&staging, &inherit_from)
        .await
        .map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;
    swap_into_place(&staging, &final_path)
        .await
        .map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;

    (p.on_progress)(P::Rescanning, None);
    Ok(entry.version.clone())
}

/// Copy a locally-staged backend into `dst`, taking only what the install
/// needs.
///
/// A `wasm` install *is* its manifest and its component — the two files [`run`]
/// writes for a registry install of that kind — so an import takes those and
/// ignores whatever sits beside them. That matters because the natural thing to
/// point `local_path` at is a source checkout, where the build tree and the
/// repository history dwarf the component by orders of magnitude and none of it
/// is ever read again.
///
/// A `subprocess` executable is not self-contained in the same way: it may need
/// siblings no manifest field declares — a bundled interpreter, shared
/// libraries — and the registry equivalent is an opaque tarball, so its tree is
/// copied whole. Only VCS metadata is dropped, being the one thing that cannot
/// be a runtime dependency.
fn copy_staged_backend(
    src: &Path,
    dst: &Path,
    kind: &str,
    entrypoint: &str,
) -> std::io::Result<()> {
    if kind == "subprocess" {
        return copy_dir_recursive(src, dst);
    }
    // Joined onto a destination below, so it may not climb out of it. The
    // manifest parser guards `[[models.files]].destination` this way; the
    // entrypoint reaches the same join and gets the same guard.
    if !super_stt_registry_types::is_safe_relative_path(entrypoint) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("refusing to import unsafe entrypoint path: {entrypoint}"),
        ));
    }
    std::fs::create_dir_all(dst)?;
    // `installed.json` is optional here: a plain local import never had an
    // asset selection to record, so most source dirs won't carry one. When
    // one is present — e.g. the source dir is itself a copy of a previously
    // installed backend — carry it forward rather than dropping it.
    let record_file = crate::registry::installed::RECORD_FILE;
    for (name, required) in [
        ("backend.toml", true),
        (entrypoint, true),
        (record_file, false),
    ] {
        let from = src.join(name);
        let to = dst.join(name);
        let meta = match from.symlink_metadata() {
            Ok(meta) => meta,
            Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        // Same policy as the tree copy: a link here would pull the target's
        // bytes into the install dir.
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("refusing to import symlink entry: {}", from.display()),
            ));
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&from, &to)?;
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        // Repository history is never a runtime dependency, and it is the
        // bulkiest thing a staged checkout carries after the build tree.
        if entry.file_name() == ".git" {
            continue;
        }
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

/// Scan the immediate children of `backends_dir` for the one whose manifest
/// declares `source`, skipping `exclude` (when given) and always skipping
/// [`STAGING_DIR_NAME`].
///
/// The `.staging` skip is an explicit rule, not an accident of directory
/// depth: a real staged install's manifest lives one level *deeper*
/// (`.staging/<id>-<version>/backend.toml`), so a naive scan happens to be
/// safe today only because `Manifest::load(backends_dir/.staging)` fails.
/// Callers that only read (like this one) would silently no-op if that
/// stopped being true; [`retire_previous_dir`], which calls `remove_dir_all`
/// on what this returns, would destroy the shared staging root instead. One
/// scan with one skip rule means there is nowhere for the two to drift apart.
///
/// A directory whose manifest will not parse is never a match: without a
/// readable `source` there is no evidence it is the same backend.
async fn find_serving(
    backends_dir: &Path,
    source: &str,
    exclude: Option<&Path>,
) -> Option<PathBuf> {
    use super_stt_registry_types::manifest::Manifest;

    let mut entries = tokio::fs::read_dir(backends_dir).await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let dir = entry.path();
        if dir.file_name().is_some_and(|n| n == STAGING_DIR_NAME) {
            continue;
        }
        if exclude.is_some_and(|e| dir == e) || !dir.is_dir() {
            continue;
        }
        if Manifest::load(&dir).is_ok_and(|m| m.backend.source == source) {
            return Some(dir);
        }
    }
    None
}

/// The directory currently serving `source`, if any — the one whose model
/// files a migrating install should inherit. Equals `final_path` for an
/// ordinary in-place update, where `final_path` is itself the directory
/// already serving `source`.
async fn previous_dir_for(backends_dir: &Path, source: &str) -> Option<PathBuf> {
    find_serving(backends_dir, source, None).await
}

/// Remove the directory that used to serve `source`, when the install just
/// landed somewhere else (a migration).
///
/// Called only after a successful swap, so the backend is already being
/// served from `keep` and removing the predecessor cannot take the last copy.
///
/// Returns the predecessor it found, or `None` when there was nothing to
/// retire. A failed `remove_dir_all` is logged but still reports the
/// predecessor: `keep` is the live directory either way, so a caller
/// repointing at it is right regardless, and the directory left behind is a
/// duplicate that the next refresh reconciles.
pub async fn retire_previous_dir(
    backends_dir: &Path,
    source: &str,
    keep: &Path,
) -> Option<PathBuf> {
    let dir = find_serving(backends_dir, source, Some(keep)).await?;
    match tokio::fs::remove_dir_all(&dir).await {
        Ok(()) => log::info!(
            "Retired {} after installing {source} into {}",
            dir.display(),
            keep.display()
        ),
        Err(e) => log::error!(
            "Failed to retire {}: {e}; it will be reconciled on a later refresh",
            dir.display()
        ),
    }
    Some(dir)
}

/// Move model files that survive the replacement from the installed directory
/// into `staging`, returning the bytes moved.
///
/// Runs before the swap so the directory that lands is already complete and
/// the swap stays atomic. Both paths live under the backends directory, so the
/// moves are same-filesystem renames rather than multi-gigabyte copies.
///
/// A missing or unparseable manifest on either side carries nothing: without
/// both file lists there is no way to tell an unchanged file from a changed
/// one, and re-downloading is the safe answer.
///
/// `carry_over::carry` moves files one at a time with no rollback, so an I/O
/// error partway through (e.g. file 3 of 5) aborts this function — and the
/// install, before the swap — with files 1–2 already moved out of
/// `final_path` and into `staging`. The live directory at `final_path` keeps
/// serving but is now missing those files; the next model load re-downloads
/// them via `usable_existing`'s hash check, so nothing wrong is ever served.
///
/// The one gap that safety net does not cover: `run`/`run_local` unconditionally
/// wipe a stale `.staging/<id>-<version>` directory before reusing it, so an
/// ordinary "retry the same version" after a partial failure destroys the
/// very files this function already carried into it — a maintainer sees the
/// update "succeed" having silently re-downloaded weights it should have
/// preserved. This is a known, accepted gap (end state is still correct
/// content), not a bug to fix here.
async fn preserve_models(staging: &Path, final_path: &Path) -> std::io::Result<u64> {
    use super_stt_registry_types::manifest::Manifest;

    let (Ok(old), Ok(new)) = (Manifest::load(final_path), Manifest::load(staging)) else {
        return Ok(0);
    };
    let keep = crate::registry::carry_over::survivors(&old, &new);
    if keep.is_empty() {
        return Ok(0);
    }
    let moved = crate::registry::carry_over::carry(final_path, staging, &keep).await?;
    log::info!(
        "Preserved {} model file(s) ({moved} bytes) across the update of {}",
        keep.len(),
        final_path.display()
    );
    Ok(moved)
}

/// Refuse to install over a directory that already serves a *different*
/// backend.
///
/// [`crate::registry::install_dir_name`] names `final_path` from the entry's
/// `backend_id`, and that value is only ever pinned to the backend it claims
/// to be on part of one route: the registry pins it for an entry whose
/// `registry.toml` row declares an `id`. A custom repository, a locally
/// staged directory, and a registry entry that predates the identifier all
/// publish whatever `[backend].id` their manifest declares — so any of them
/// can name a directory another backend is already installed in.
/// [`swap_into_place`] renames whatever sits there aside and deletes it, so
/// without this check an install could destroy an unrelated backend and the
/// multi-gigabyte model files under it. `source` is the identity the daemon
/// resolves backends and models by, so it is what identity is judged on here.
///
/// A directory whose manifest does not parse is not a claim of identity and
/// does not block the install: replacing a half-written or corrupt install is
/// the ordinary repair path, and refusing it would leave the user with no way
/// forward.
///
/// # Errors
/// Returns `(Installing, InstallDirConflict)` when `final_path` holds a
/// manifest declaring a different `source`.
fn ensure_dir_free_for(
    final_path: &Path,
    source: &str,
) -> Result<(), (InstallPhase, InstallError)> {
    use super_stt_registry_types::manifest::Manifest;

    let Ok(occupant) = Manifest::load(final_path) else {
        return Ok(());
    };
    if occupant.backend.source == source {
        return Ok(());
    }
    log::error!(
        "Refusing to install {source} into {}: that directory already serves {}",
        final_path.display(),
        occupant.backend.source
    );
    Err((InstallPhase::Installing, InstallError::InstallDirConflict))
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
    use crate::registry::install_dir_name;

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
            backend_id: None,
            source: "github.com/x/y".into(),
            version: "1.0.0".into(),
            tag: "v1.0.0".into(),
            name: "X".into(),
            description: None,
            license: String::new(),
            kind: "subprocess".into(),
            contract: "v1".into(),
            min_client: None,
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

    fn index_entry(id: &str, backend_id: Option<&str>) -> IndexBackend {
        let mut e = minimal_entry();
        e.id = id.to_string();
        e.backend_id = backend_id.map(str::to_string);
        e
    }

    #[test]
    fn the_install_dir_is_the_backend_id_when_one_is_declared() {
        let e = index_entry("voxtral", Some("app.super-stt.voxtral"));
        assert_eq!(install_dir_name(&e), "app.super-stt.voxtral");
    }

    #[test]
    fn the_install_dir_falls_back_to_the_registry_key() {
        let e = index_entry("voxtral", None);
        assert_eq!(install_dir_name(&e), "voxtral");
    }

    /// `backend_id` arrives from `index.json` over the network. Even if the
    /// registry-client boundary (`retain_safe_backends`) failed to sanitize
    /// it, `install_dir_name` must never hand back a value that would let the
    /// caller's `backends_dir.join(...)` escape the backends directory.
    #[test]
    fn install_dir_name_falls_back_when_backend_id_is_unsafe() {
        for unsafe_id in [
            "..",
            "../../../../home/jorge/.ssh",
            "/etc/passwd",
            "a/b",
            "",
        ] {
            let e = index_entry("voxtral", Some(unsafe_id));
            assert_eq!(
                install_dir_name(&e),
                "voxtral",
                "unsafe backend_id {unsafe_id:?} must not be used"
            );
        }
    }

    /// `.staging` is a legal path component, so a component-level safety
    /// check waves it through — but it names the shared staging root every
    /// install writes into, and resolving an install directory to it would
    /// point the swap at every in-flight install at once. `backend_id` is
    /// therefore held to the full `[backend].id` format rule, which
    /// `.staging` fails (a leading dot, and one segment).
    #[test]
    fn install_dir_name_rejects_the_shared_staging_root() {
        assert!(
            super_stt_shared::registry::is_safe_component(".staging"),
            "the premise: a component-level check accepts .staging"
        );
        let e = index_entry("voxtral", Some(".staging"));
        assert_eq!(install_dir_name(&e), "voxtral");
    }

    /// More broadly: `index.json` is the only route into an install directory
    /// name that does not pass through `Manifest::parse`, so it must apply the
    /// same `[backend].id` rule rather than a looser one.
    #[test]
    fn install_dir_name_rejects_a_malformed_backend_id() {
        for malformed in [
            ".staging",
            "voxtral",
            "app.voxtral",
            "App.Super-STT.Voxtral",
            "app.super_stt.voxtral",
            "app..voxtral",
            "app.super-stt.voxtral-",
        ] {
            let e = index_entry("registry-key", Some(malformed));
            assert_eq!(
                install_dir_name(&e),
                "registry-key",
                "malformed backend_id {malformed:?} must not name a directory"
            );
        }
    }

    /// The same traversal cases, but proving the property that actually
    /// matters: joining the returned name onto the backends dir never
    /// produces a path outside it.
    #[test]
    fn install_dir_name_never_escapes_the_backends_dir_when_joined() {
        let backends_dir = Path::new("/var/lib/super-stt/backend/list");
        for unsafe_id in ["..", "../../../../home/jorge/.ssh", "/etc/passwd", "a/b"] {
            let e = index_entry("voxtral", Some(unsafe_id));
            let joined = backends_dir.join(install_dir_name(&e));
            assert!(
                joined.starts_with(backends_dir),
                "backend_id {unsafe_id:?} escaped: {}",
                joined.display()
            );
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

    /// Lay out the shape an operator actually points `local_path` at: a source
    /// checkout, where the component sits beside a build tree and a `.git`.
    fn staged_checkout() -> tempfile::TempDir {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("backend.toml"), b"stub").unwrap();
        std::fs::write(src.path().join("y.wasm"), b"component").unwrap();
        std::fs::write(src.path().join("README.md"), b"docs").unwrap();
        std::fs::create_dir_all(src.path().join("target/wasm32-wasip2/release")).unwrap();
        std::fs::write(
            src.path().join("target/wasm32-wasip2/release/y.wasm"),
            b"build tree",
        )
        .unwrap();
        std::fs::create_dir_all(src.path().join(".git/objects")).unwrap();
        std::fs::write(src.path().join(".git/objects/pack"), b"history").unwrap();
        src
    }

    /// A wasm install is its manifest and its component; an import copies those
    /// and nothing else. The build tree and the history beside them are what
    /// make a naive tree copy orders of magnitude larger than the install.
    #[test]
    fn wasm_import_copies_only_the_manifest_and_the_component() {
        let src = staged_checkout();
        let dst = tempfile::tempdir().unwrap();
        let out = dst.path().join("out");
        copy_staged_backend(src.path(), &out, "wasm", "y.wasm").unwrap();

        assert_eq!(std::fs::read(out.join("y.wasm")).unwrap(), b"component");
        assert!(out.join("backend.toml").is_file());
        let mut copied: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        copied.sort();
        assert_eq!(copied, ["backend.toml", "y.wasm"]);
    }

    /// A source dir that is itself a copy of a previously installed backend
    /// carries its `installed.json` forward instead of it being silently
    /// dropped, the one entry in the allowlist that is optional rather than
    /// required.
    #[test]
    fn local_import_carries_forward_an_existing_installed_json() {
        let src = staged_checkout();
        let record = br#"{"selected":{"target":"x86_64-unknown-linux-gnu","accel":["cuda"]}}"#;
        std::fs::write(
            src.path().join(crate::registry::installed::RECORD_FILE),
            record,
        )
        .unwrap();
        let dst = tempfile::tempdir().unwrap();
        let out = dst.path().join("out");
        copy_staged_backend(src.path(), &out, "wasm", "y.wasm").unwrap();

        assert_eq!(
            std::fs::read(out.join(crate::registry::installed::RECORD_FILE)).unwrap(),
            record
        );
    }

    /// A subprocess executable may need siblings no manifest field declares, so
    /// its tree is taken whole — except the history, which cannot be one.
    #[test]
    fn subprocess_import_keeps_the_tree_but_drops_vcs_metadata() {
        let src = staged_checkout();
        let dst = tempfile::tempdir().unwrap();
        let out = dst.path().join("out");
        copy_staged_backend(src.path(), &out, "subprocess", "y.wasm").unwrap();

        assert!(out.join("README.md").is_file());
        assert!(out.join("target/wasm32-wasip2/release/y.wasm").is_file());
        assert!(!out.join(".git").exists());
    }

    /// A nested entrypoint (`bin/qwen3-asr`) needs its parent created.
    #[test]
    fn wasm_import_creates_the_entrypoints_parent() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("backend.toml"), b"stub").unwrap();
        std::fs::create_dir(src.path().join("bin")).unwrap();
        std::fs::write(src.path().join("bin/y.wasm"), b"component").unwrap();
        let dst = tempfile::tempdir().unwrap();
        let out = dst.path().join("out");
        copy_staged_backend(src.path(), &out, "wasm", "bin/y.wasm").unwrap();
        assert_eq!(std::fs::read(out.join("bin/y.wasm")).unwrap(), b"component");
    }

    /// The entrypoint is joined onto the destination, so a traversing value
    /// would write outside the staging dir.
    #[test]
    fn wasm_import_rejects_an_escaping_entrypoint() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("backend.toml"), b"stub").unwrap();
        let dst = tempfile::tempdir().unwrap();
        let err = copy_staged_backend(
            src.path(),
            &dst.path().join("out"),
            "wasm",
            "../escaped.wasm",
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn wasm_import_rejects_a_symlinked_entrypoint() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("backend.toml"), b"stub").unwrap();
        std::os::unix::fs::symlink("/etc/hostname", src.path().join("y.wasm")).unwrap();
        let dst = tempfile::tempdir().unwrap();
        let err =
            copy_staged_backend(src.path(), &dst.path().join("out"), "wasm", "y.wasm").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    /// Defect 2: replacing a backend directory took its downloaded weights
    /// with it. The staged manifest declares the same file at the same URL, so
    /// the bytes move across instead of re-downloading.
    #[tokio::test]
    async fn preserve_models_moves_unchanged_weights_into_staging() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join(".staging/y-1.0.1");
        let final_path = root.path().join("y");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(final_path.join("models/m")).unwrap();

        let toml = |version: &str| {
            format!(
                r#"
[backend]
    source     = "github.com/x/y"
    name       = "Y"
    version    = "{version}"
    kind       = "subprocess"
    entrypoint = "y"
    contract   = "v1"
    license    = "Apache-2.0"
    description = "Test backend."

[[assets.subprocess]]
    file   = "y.tar.gz"
    target = "x86_64-unknown-linux-gnu"
    accel  = ["cpu"]

[[models]]
    name                = "m"
    primary_language    = "en"
    supported_languages = ["en"]
    supported_devices   = ["cpu"]
    files = [
        {{ url = "https://h/a.bin", destination = "models/m/a.bin" }},
    ]
"#
            )
        };
        std::fs::write(final_path.join("backend.toml"), toml("1.0.0")).unwrap();
        std::fs::write(staging.join("backend.toml"), toml("1.0.1")).unwrap();
        std::fs::write(final_path.join("models/m/a.bin"), b"weights").unwrap();

        let moved = super::preserve_models(&staging, &final_path).await.unwrap();
        assert_eq!(moved, 7);
        assert!(staging.join("models/m/a.bin").exists());
    }

    #[tokio::test]
    async fn preserve_models_is_a_no_op_without_an_installed_manifest() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let moved = super::preserve_models(&staging, &root.path().join("absent"))
            .await
            .unwrap();
        assert_eq!(moved, 0);
    }

    /// The other half of the predicate's contract at this integration layer:
    /// a file whose `url` changed between the two manifests must be left
    /// behind under the installed dir, not carried into staging, so the next
    /// model load re-downloads the new bytes instead of serving stale ones.
    #[tokio::test]
    async fn preserve_models_leaves_a_changed_url_behind() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join(".staging/y-1.0.1");
        let final_path = root.path().join("y");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(final_path.join("models/m")).unwrap();

        let toml = |version: &str, url: &str| {
            format!(
                r#"
[backend]
    source     = "github.com/x/y"
    name       = "Y"
    version    = "{version}"
    kind       = "subprocess"
    entrypoint = "y"
    contract   = "v1"
    license    = "Apache-2.0"
    description = "Test backend."

[[assets.subprocess]]
    file   = "y.tar.gz"
    target = "x86_64-unknown-linux-gnu"
    accel  = ["cpu"]

[[models]]
    name                = "m"
    primary_language    = "en"
    supported_languages = ["en"]
    supported_devices   = ["cpu"]
    files = [
        {{ url = "{url}", destination = "models/m/a.bin" }},
    ]
"#
            )
        };
        std::fs::write(
            final_path.join("backend.toml"),
            toml("1.0.0", "https://h/a.bin"),
        )
        .unwrap();
        std::fs::write(
            staging.join("backend.toml"),
            toml("1.0.1", "https://h/a-v2.bin"),
        )
        .unwrap();
        std::fs::write(final_path.join("models/m/a.bin"), b"weights").unwrap();

        let moved = super::preserve_models(&staging, &final_path).await.unwrap();
        assert_eq!(moved, 0);
        assert!(
            final_path.join("models/m/a.bin").exists(),
            "the old file must stay put so the daemon re-downloads the new URL"
        );
        assert!(!staging.join("models/m/a.bin").exists());
    }

    /// A backend directory holding `source`, with one downloaded weight file
    /// under it — the thing an unguarded swap would delete.
    fn occupied_backend_dir(dir: &Path, source: &str) {
        std::fs::create_dir_all(dir.join("models/m")).unwrap();
        std::fs::write(
            dir.join("backend.toml"),
            format!(
                r#"
[backend]
source = "{source}"
name = "Occupant"
version = "1.0.0"
kind = "subprocess"
entrypoint = "occupant"
contract = "v1"
description = "Test backend."
"#
            ),
        )
        .unwrap();
        std::fs::write(dir.join("models/m/a.bin"), b"expensive weights").unwrap();
    }

    #[test]
    fn ensure_dir_free_for_allows_an_update_of_the_same_backend() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("app.super-stt.voxtral");
        occupied_backend_dir(&dir, "github.com/x/voxtral");
        ensure_dir_free_for(&dir, "github.com/x/voxtral").expect("same source is an update");
    }

    /// Replacing a half-written or corrupt install is the ordinary repair
    /// path: a directory whose manifest does not parse makes no claim of
    /// identity, so it must not block an install.
    #[test]
    fn ensure_dir_free_for_allows_an_unreadable_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("app.super-stt.voxtral");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("backend.toml"), b"this is not toml = = =").unwrap();
        ensure_dir_free_for(&dir, "github.com/x/voxtral")
            .expect("a corrupt install is replaceable");

        let empty = root.path().join("brand-new");
        ensure_dir_free_for(&empty, "github.com/x/voxtral").expect("a fresh install has no dir");
    }

    #[test]
    fn ensure_dir_free_for_refuses_a_directory_serving_another_backend() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("app.super-stt.voxtral");
        occupied_backend_dir(&dir, "github.com/x/voxtral");
        let err = ensure_dir_free_for(&dir, "github.com/someone/thing").unwrap_err();
        assert_eq!(err.1, InstallError::InstallDirConflict);
    }

    /// The whole reason the guard exists. `install_dir_name` reads
    /// `backend_id`, which nothing pins to the backend that declares it on
    /// the local-dir route — an operator-staged `backend.toml` can name any
    /// `[backend].id` it likes. Landing on another backend's directory must
    /// fail the install outright, because `swap_into_place` would otherwise
    /// rename that directory aside and delete it, weights and all.
    #[tokio::test]
    async fn run_local_refuses_to_install_over_a_different_backend() {
        let root = tempfile::tempdir().unwrap();
        let backends_dir = root.path().join("backends");
        let victim = backends_dir.join("app.super-stt.voxtral");
        occupied_backend_dir(&victim, "github.com/x/voxtral");
        let victim_manifest = std::fs::read(victim.join("backend.toml")).unwrap();

        // The staged import: a different backend, but claiming Voxtral's
        // install directory via `backend_id`.
        let staged = root.path().join("staged");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(
            staged.join("backend.toml"),
            r#"
[backend]
source = "github.com/someone/thing"
id = "app.super-stt.voxtral"
name = "Thing"
version = "9.9.9"
kind = "subprocess"
entrypoint = "thing"
contract = "v1"
description = "Test backend."
"#,
        )
        .unwrap();
        std::fs::write(staged.join("thing"), b"#!/bin/sh\n").unwrap();

        let mut entry = minimal_entry();
        entry.id = "thing".into();
        entry.backend_id = Some("app.super-stt.voxtral".into());
        entry.source = "github.com/someone/thing".into();
        entry.entrypoint = "thing".into();

        let pipeline = Pipeline {
            backends_dir: backends_dir.clone(),
            cache_dir: root.path().join("cache"),
            http: reqwest::Client::new(),
            on_progress: Arc::new(|_, _| {}),
        };
        let err = run_local(&pipeline, &entry, &staged)
            .await
            .expect_err("installing over another backend must be refused");

        assert_eq!(err.1, InstallError::InstallDirConflict);
        assert_eq!(err.0, InstallPhase::Installing);
        assert!(
            victim.join("models/m/a.bin").exists(),
            "the occupant's downloaded weights must survive untouched"
        );
        assert_eq!(
            std::fs::read(victim.join("backend.toml")).unwrap(),
            victim_manifest,
            "the occupant's manifest must be exactly as it was"
        );
        let mut sidecar = victim.as_os_str().to_owned();
        sidecar.push(".old");
        assert!(
            !Path::new(&sidecar).exists(),
            "the swap must not have started"
        );
    }

    /// The same guard on the registry/custom-repo route, which reaches the
    /// swap through [`run`] rather than [`run_local`]. Everything up to the
    /// install directory is real: the asset and the pinned manifest are
    /// downloaded and verified before the conflict is detected.
    #[tokio::test]
    async fn run_refuses_to_install_over_a_different_backend() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let root = tempfile::tempdir().unwrap();
        let backends_dir = root.path().join("backends");
        let victim = backends_dir.join("app.super-stt.voxtral");
        occupied_backend_dir(&victim, "github.com/x/voxtral");

        let component = b"\0asm-not-really".to_vec();
        let manifest = r#"
[backend]
source = "github.com/someone/thing"
id = "app.super-stt.voxtral"
name = "Thing"
version = "9.9.9"
kind = "wasm"
entrypoint = "thing.wasm"
contract = "v1"
description = "Test backend."

[[models]]
name = "m"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]
"#;
        let mut server = mockito::Server::new_async().await;
        let asset_mock = server
            .mock("GET", "/thing.wasm")
            .with_status(200)
            .with_body(component.as_slice())
            .create_async()
            .await;
        let manifest_mock = server
            .mock("GET", "/backend.toml")
            .with_status(200)
            .with_body(manifest)
            .create_async()
            .await;

        let mut entry = minimal_entry();
        entry.id = "thing".into();
        // The poisoned value: a legitimate `source`, but an `id` naming a
        // directory that belongs to someone else.
        entry.backend_id = Some("app.super-stt.voxtral".into());
        entry.source = "github.com/someone/thing".into();
        entry.kind = "wasm".into();
        entry.entrypoint = "thing.wasm".into();
        entry.version = "9.9.9".into();
        entry.assets.wasm = Some(IndexAsset {
            url: format!("{}/thing.wasm", server.url()),
            size: component.len() as u64,
            sha256: sha_hex(&component),
        });
        entry.manifest = Some(IndexAsset {
            url: format!("{}/backend.toml", server.url()),
            size: manifest.len() as u64,
            sha256: sha_hex(manifest.as_bytes()),
        });

        let pipeline = Pipeline {
            backends_dir: backends_dir.clone(),
            cache_dir: root.path().join("cache"),
            http: reqwest::Client::new(),
            on_progress: Arc::new(|_, _| {}),
        };
        let err = run(&pipeline, &entry, &Selection::Wasm)
            .await
            .expect_err("installing over another backend must be refused");

        asset_mock.assert_async().await;
        manifest_mock.assert_async().await;
        assert_eq!(err.1, InstallError::InstallDirConflict);
        assert!(
            victim.join("models/m/a.bin").exists(),
            "the occupant's downloaded weights must survive untouched"
        );
        assert!(
            !victim.join("thing.wasm").exists(),
            "nothing from the refused install may reach the occupant's directory"
        );
    }

    #[tokio::test]
    async fn retire_previous_dir_removes_the_superseded_directory() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("super-stt-voxtral");
        let new = root.path().join("app.super-stt.voxtral");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(
            old.join("backend.toml"),
            r#"
[backend]
source = "github.com/x/super-stt-voxtral"
name = "Voxtral"
version = "1.0.0"
kind = "subprocess"
entrypoint = "voxtral"
contract = "v1"
description = "Test backend."
"#,
        )
        .unwrap();

        let removed =
            super::retire_previous_dir(root.path(), "github.com/x/super-stt-voxtral", &new).await;

        assert_eq!(removed.as_deref(), Some(old.as_path()));
        assert!(!old.exists(), "the superseded directory is gone");
        assert!(new.exists(), "the new directory survives");
    }

    #[tokio::test]
    async fn retire_previous_dir_never_removes_the_directory_it_was_told_to_keep() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("app.super-stt.voxtral");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("backend.toml"),
            r#"
[backend]
source = "github.com/x/super-stt-voxtral"
name = "Voxtral"
version = "1.0.0"
kind = "subprocess"
entrypoint = "voxtral"
contract = "v1"
description = "Test backend."
"#,
        )
        .unwrap();

        let removed =
            super::retire_previous_dir(root.path(), "github.com/x/super-stt-voxtral", &dir).await;

        assert!(removed.is_none());
        assert!(dir.exists());
    }

    /// A directory whose manifest fails to parse carries no evidence it is the
    /// same backend, so it must never be treated as a retirement candidate —
    /// even if it happens to sit alongside `keep` and nothing else claims the
    /// source.
    #[tokio::test]
    async fn retire_previous_dir_skips_a_directory_whose_manifest_does_not_parse() {
        let root = tempfile::tempdir().unwrap();
        let unparseable = root.path().join("mystery-dir");
        let keep = root.path().join("app.super-stt.voxtral");
        std::fs::create_dir_all(&unparseable).unwrap();
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(unparseable.join("backend.toml"), "this is not toml = = =").unwrap();

        let removed =
            super::retire_previous_dir(root.path(), "github.com/x/super-stt-voxtral", &keep).await;

        assert!(removed.is_none());
        assert!(
            unparseable.exists(),
            "a directory with an unparseable manifest must never be retired"
        );
    }

    #[tokio::test]
    async fn previous_dir_for_finds_the_directory_serving_a_source() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("super-stt-voxtral");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(
            old.join("backend.toml"),
            r#"
[backend]
source = "github.com/x/super-stt-voxtral"
name = "Voxtral"
version = "1.0.0"
kind = "subprocess"
entrypoint = "voxtral"
contract = "v1"
description = "Test backend."
"#,
        )
        .unwrap();

        let found = super::previous_dir_for(root.path(), "github.com/x/super-stt-voxtral").await;

        assert_eq!(found.as_deref(), Some(old.as_path()));
    }

    #[tokio::test]
    async fn previous_dir_for_is_none_when_no_directory_serves_the_source() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("something-else")).unwrap();

        let found = super::previous_dir_for(root.path(), "github.com/x/super-stt-voxtral").await;

        assert!(found.is_none());
    }

    /// `.staging` (the shared root every install/update stages into) sits one
    /// level inside `backends_dir`, exactly where a real backend directory
    /// would. Both scans are safe today only because a real staged install's
    /// manifest lives one level *deeper* (`.staging/<id>-<version>/backend.toml`),
    /// so a plain `Manifest::load(backends_dir/.staging)` fails. Nothing
    /// enforces that shape, so this proves `.staging` is skipped by an
    /// explicit rule rather than by accident — a manifest ever placed one
    /// level shallower (a bug, a manual recovery) must never turn into
    /// `retire_previous_dir` calling `remove_dir_all` on the shared staging
    /// root out from under every in-flight install.
    #[tokio::test]
    async fn the_staging_root_is_never_matched_as_a_backend_directory() {
        let root = tempfile::tempdir().unwrap();
        let staging_root = root.path().join(super::STAGING_DIR_NAME);
        std::fs::create_dir_all(&staging_root).unwrap();
        std::fs::write(
            staging_root.join("backend.toml"),
            r#"
[backend]
source = "github.com/x/super-stt-voxtral"
name = "Voxtral"
version = "1.0.0"
kind = "subprocess"
entrypoint = "voxtral"
contract = "v1"
description = "Test backend."
"#,
        )
        .unwrap();
        let keep = root.path().join("app.super-stt.voxtral");
        std::fs::create_dir_all(&keep).unwrap();

        let found = super::previous_dir_for(root.path(), "github.com/x/super-stt-voxtral").await;
        assert!(
            found.is_none(),
            ".staging must never be matched as a backend directory"
        );

        let removed =
            super::retire_previous_dir(root.path(), "github.com/x/super-stt-voxtral", &keep).await;
        assert!(removed.is_none());
        assert!(
            staging_root.exists(),
            "the shared staging root must never be removed"
        );
    }

    /// The 8.8 GB case: a migration where `final_path` does not exist yet must
    /// still carry the old directory's unchanged model files into staging, by
    /// resolving the inherit-from directory through `previous_dir_for` rather
    /// than comparing straight against `final_path`.
    #[tokio::test]
    async fn a_migration_preserves_model_files_via_previous_dir_for() {
        let root = tempfile::tempdir().unwrap();
        let old_dir = root.path().join("super-stt-voxtral");
        let staging = root.path().join(".staging/app.super-stt.voxtral-1.0.1");
        let final_path = root.path().join("app.super-stt.voxtral");
        std::fs::create_dir_all(old_dir.join("models/m")).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        assert!(!final_path.exists(), "final_path must not exist yet");

        let toml = |version: &str| {
            format!(
                r#"
[backend]
    source     = "github.com/x/voxtral"
    name       = "Voxtral"
    version    = "{version}"
    kind       = "subprocess"
    entrypoint = "voxtral"
    contract   = "v1"
    license    = "Apache-2.0"
    description = "Test backend."

[[assets.subprocess]]
    file   = "voxtral.tar.gz"
    target = "x86_64-unknown-linux-gnu"
    accel  = ["cpu"]

[[models]]
    name                = "m"
    primary_language    = "en"
    supported_languages = ["en"]
    supported_devices   = ["cpu"]
    files = [
        {{ url = "https://h/a.bin", destination = "models/m/a.bin" }},
    ]
"#
            )
        };
        std::fs::write(old_dir.join("backend.toml"), toml("1.0.0")).unwrap();
        std::fs::write(staging.join("backend.toml"), toml("1.0.1")).unwrap();
        std::fs::write(old_dir.join("models/m/a.bin"), b"weights").unwrap();

        let inherit_from = super::previous_dir_for(root.path(), "github.com/x/voxtral")
            .await
            .unwrap_or_else(|| final_path.clone());
        assert_eq!(inherit_from, old_dir, "must inherit from the old directory");

        let moved = super::preserve_models(&staging, &inherit_from)
            .await
            .unwrap();

        assert_eq!(moved, 7);
        assert!(
            staging.join("models/m/a.bin").exists(),
            "the model file must survive the migration"
        );
        assert_eq!(
            std::fs::read(staging.join("models/m/a.bin")).unwrap(),
            b"weights",
            "the carried file's bytes, not just its existence, must survive"
        );
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
