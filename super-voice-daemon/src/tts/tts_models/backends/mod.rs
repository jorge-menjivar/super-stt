// SPDX-License-Identifier: GPL-3.0-only
//! Backend discovery: `super_engine_daemon::backends`, shared with Super STT,
//! for Super TTS's manifests.
//!
//! Models are served by out-of-tree backends installed under a backends
//! directory. Each backend is a subdirectory containing a `backend.toml`
//! manifest (see [`manifest`]) plus its entrypoint (a `.wasm` component or a
//! native binary). [`discover`] scans that directory and turns each manifest
//! into a [`DiscoveredBackend`] carrying fully-resolved
//! [`ModelDefinition`](crate::tts::tts_models::ModelDefinition)s.

pub mod manifest;

use std::path::{Path, PathBuf};

use super_engine_daemon::backends as engine;
pub use super_engine_daemon::backends::{dir_name, find_model, list_models};
use super_voice_registry_types::Voice;

/// A backend discovered on disk. See
/// `super_engine_daemon::backends::DiscoveredBackend`.
pub type DiscoveredBackend = engine::DiscoveredBackend<Voice>;

/// Read a backend's version from the `backend.toml` in `dir`. See
/// `super_engine_daemon::backends::installed_version`.
#[must_use]
pub fn installed_version(dir: &Path) -> Option<String> {
    engine::installed_version::<Voice>(dir)
}

/// Scan `backends_dir` for installed backends, returning the one serving
/// each `source` and the duplicates it supersedes. See
/// `super_engine_daemon::backends::discover`.
#[must_use]
///
/// Only this daemon's models are kept: one Super Voice backend may also hold
/// models of the other feature, which the other daemon serves. A backend with
/// none of this daemon's is left out.
pub fn discover(backends_dir: &Path) -> (Vec<DiscoveredBackend>, Vec<DiscoveredBackend>) {
    let (live, superseded) = engine::discover::<Voice>(backends_dir);
    let live = live
        .into_iter()
        .filter_map(|mut backend| {
            backend.models.retain(|m| m.product.role.is_synthesis());
            (!backend.models.is_empty()).then_some(backend)
        })
        .collect();
    (live, superseded)
}

/// The default backends search directory: the daemon's one,
/// `<data_dir>/super-voice/backends`.
#[must_use]
pub fn default_backends_dir() -> PathBuf {
    crate::stt_models::backends::default_backends_dir()
}

/// Text to speech's backends among `all`: each with its synthesis models
/// only, and none that has no synthesis model.
#[must_use]
pub fn synthesis(all: &[DiscoveredBackend]) -> Vec<DiscoveredBackend> {
    all.iter()
        .filter_map(|backend| {
            let mut backend = backend.clone();
            backend.models.retain(|m| m.product.role.is_synthesis());
            (!backend.models.is_empty()).then_some(backend)
        })
        .collect()
}
