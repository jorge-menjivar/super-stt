// SPDX-License-Identifier: GPL-3.0-only
//! Backend discovery: `super_engine_daemon::backends`, shared with Super TTS,
//! for Super STT's manifests.
//!
//! Models are served by out-of-tree backends installed under a backends
//! directory. Each backend is a subdirectory containing a `backend.toml`
//! manifest (see [`manifest`]) plus its entrypoint (a `.wasm` component or a
//! native binary). [`discover`] scans that directory and turns each manifest
//! into a [`DiscoveredBackend`] carrying fully-resolved
//! [`ModelDefinition`](crate::stt_models::ModelDefinition)s.

pub(crate) mod base_url;
pub mod manifest;

use std::path::{Path, PathBuf};

use super_engine_daemon::backends as engine;
pub use super_engine_daemon::backends::{dir_name, find_model, list_models};
use super_stt_registry_types::Stt;

/// A backend discovered on disk. See
/// `super_engine_daemon::backends::DiscoveredBackend`.
pub type DiscoveredBackend = engine::DiscoveredBackend<Stt>;

/// Read a backend's version from the `backend.toml` in `dir`. See
/// `super_engine_daemon::backends::installed_version`.
#[must_use]
pub fn installed_version(dir: &Path) -> Option<String> {
    engine::installed_version::<Stt>(dir)
}

/// Scan `backends_dir` for installed backends, returning the one serving
/// each `source` and the duplicates it supersedes. See
/// `super_engine_daemon::backends::discover`.
#[must_use]
pub fn discover(backends_dir: &Path) -> (Vec<DiscoveredBackend>, Vec<DiscoveredBackend>) {
    engine::discover::<Stt>(backends_dir)
}

/// The default backends search directory: `<data_dir>/super-stt/backends`.
#[must_use]
pub fn default_backends_dir() -> PathBuf {
    engine::default_backends_dir(&super_stt_shared::SUPER_STT)
}
