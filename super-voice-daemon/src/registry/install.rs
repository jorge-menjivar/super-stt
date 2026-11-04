// SPDX-License-Identifier: GPL-3.0-only
//! Install pipeline: `super_engine_daemon::registry::install`, for Super STT's
//! manifests. State machine: Resolving → Downloading → Verifying → Extracting →
//! Installing → Rescanning → Done | Failed.

use std::path::{Path, PathBuf};

use super_engine_daemon::registry::install as engine;
pub use super_engine_daemon::registry::install::{Pipeline, PipelineError};
use super_stt_shared::registry::events::{InstallError, InstallPhase};
use super_voice_registry_types::Voice;

use crate::registry::compat::Selection;
use crate::registry::index_schema::IndexBackend;

/// Run an install. See `super_engine_daemon::registry::install::run`.
///
/// # Errors
/// Returns `(InstallPhase, InstallError)` on failure.
pub async fn run<F>(
    p: &Pipeline<F>,
    entry: &IndexBackend,
    selection: &Selection,
) -> Result<String, (InstallPhase, InstallError)>
where
    F: Fn(InstallPhase, Option<(u64, Option<u64>)>) + Send + Sync,
{
    engine::run::<Voice, F>(p, entry, selection).await
}

/// Install from a staged local directory. See
/// `super_engine_daemon::registry::install::run_local`.
///
/// # Errors
/// Returns `(InstallPhase, InstallError)` on failure.
pub async fn run_local<F>(
    p: &Pipeline<F>,
    entry: &IndexBackend,
    src_dir: &Path,
) -> Result<String, (InstallPhase, InstallError)>
where
    F: Fn(InstallPhase, Option<(u64, Option<u64>)>) + Send + Sync,
{
    engine::run_local::<Voice, F>(p, entry, src_dir).await
}

/// Remove the directory that used to serve `source`. See
/// `super_engine_daemon::registry::install::retire_previous_dir`.
pub async fn retire_previous_dir(
    backends_dir: &Path,
    source: &str,
    keep: &Path,
) -> Option<PathBuf> {
    engine::retire_previous_dir::<Voice>(backends_dir, source, keep).await
}
