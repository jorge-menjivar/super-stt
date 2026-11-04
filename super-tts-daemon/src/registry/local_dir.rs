// SPDX-License-Identifier: GPL-3.0-only
//! Resolve a locally-staged backend directory into an [`IndexBackend`] for the
//! Import-from-dir install path: `super_engine_daemon::registry::local_dir`,
//! shared with Super STT, for Super TTS's manifests.

use std::path::Path;

pub use super_engine_daemon::registry::local_dir::ResolveError;

use crate::registry::index_schema::IndexBackend;

/// Read and validate `<local_path>/backend.toml` into an index entry. See
/// `super_engine_daemon::registry::local_dir::resolve`.
///
/// # Errors
/// See [`ResolveError`].
pub fn resolve(local_path: &Path) -> Result<IndexBackend, ResolveError> {
    super_engine_daemon::registry::local_dir::resolve::<super_tts_registry_types::Tts>(local_path)
}
