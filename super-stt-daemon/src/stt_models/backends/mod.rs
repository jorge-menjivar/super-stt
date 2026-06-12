// SPDX-License-Identifier: GPL-3.0-only
//! Backend discovery.
//!
//! Models are served by out-of-tree backends installed under a backends
//! directory. Each backend is a subdirectory containing a `backend.toml`
//! manifest (see [`manifest`]) plus its entrypoint (a `.wasm` component or a
//! native binary). This module scans that directory and turns each manifest
//! into a [`DiscoveredBackend`] carrying fully-resolved [`ModelDefinition`]s.

pub mod manifest;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use log::{error, info, warn};

use super_stt_shared::models::provider::Provider;
use super_stt_shared::models::registry::ModelDefinition;

use manifest::{Device, Manifest, ModelEntry, Opt, Secret};

/// A backend discovered on disk, with the models it serves resolved into
/// [`ModelDefinition`]s keyed by `(name, provider, source)`.
#[derive(Clone, Debug)]
pub struct DiscoveredBackend {
    /// Directory holding `backend.toml` and the entrypoint.
    pub dir: PathBuf,
    /// Repo id (`[backend].source`); the `source` of every model it serves.
    pub source: String,
    /// Human-facing backend name (`[backend].name`).
    pub name: String,
    /// `"wasm"` or `"subprocess"`.
    pub kind: String,
    /// Entrypoint relative to `dir` (component file or binary).
    pub entrypoint: String,
    /// Hosts the backend is permitted to reach (`[network].allowed_hosts`).
    pub allowed_hosts: Vec<String>,
    /// Declared secrets the backend expects as `x-stt-secret-*` headers.
    pub secrets: Vec<Secret>,
    /// Declared options the backend accepts as `x-stt-option-*` headers.
    pub options: Vec<Opt>,
    /// Models this backend serves.
    pub models: Vec<ModelDefinition>,
}

/// Scan `backends_dir` for installed backends. Subdirectories without a
/// readable, parseable `backend.toml` are skipped with a warning.
#[must_use]
pub fn discover(backends_dir: &Path) -> Vec<DiscoveredBackend> {
    let entries = match std::fs::read_dir(backends_dir) {
        Ok(e) => e,
        Err(e) => {
            info!(
                "Backends directory {} not readable ({e}); no backends discovered",
                backends_dir.display()
            );
            return Vec::new();
        }
    };

    let mut backends = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() || !dir.join("backend.toml").exists() {
            continue;
        }
        match load_backend(&dir) {
            Ok(backend) => {
                info!(
                    "Discovered backend '{}' ({}) serving {} model(s) from {}",
                    backend.name,
                    backend.source,
                    backend.models.len(),
                    dir.display()
                );
                backends.push(backend);
            }
            Err(e) => warn!("Skipping backend at {}: {e:#}", dir.display()),
        }
    }
    dedup_sources(backends)
}

/// Enforce that each discovered backend has a unique `source`. The `source` is
/// the stable identity the daemon resolves both models and the *active backend*
/// by, so a collision would make selection ambiguous — the first match would
/// silently win (e.g. selecting Voxtral could activate Mistral if both declared
/// the same source). Keep the first occurrence of each source and drop the rest
/// with a loud error so the misconfiguration is visible rather than silent.
fn dedup_sources(backends: Vec<DiscoveredBackend>) -> Vec<DiscoveredBackend> {
    let mut seen: HashSet<String> = HashSet::with_capacity(backends.len());
    let mut out = Vec::with_capacity(backends.len());
    for b in backends {
        if seen.insert(b.source.clone()) {
            out.push(b);
        } else {
            error!(
                "Backend '{}' at {} declares source '{}', which is already used by \
                 another discovered backend; skipping it. Each backend.toml must \
                 declare a unique [backend].source.",
                b.name,
                b.dir.display(),
                b.source
            );
        }
    }
    out
}

/// Parse one backend directory into a [`DiscoveredBackend`].
///
/// Per-model errors (missing/invalid `supported_devices`) are fatal for the
/// *whole* backend: discovery skips it rather than expose a half-defined
/// model. Unknown providers and devices are rejected at parse time by the
/// typed manifest; this function enforces the cross-field device rules via
/// [`validate_supported_devices`].
fn load_backend(dir: &Path) -> anyhow::Result<DiscoveredBackend> {
    let m = Manifest::load(dir)?;
    manifest::validate_runtime(&m)?;
    let source = m.backend.source.clone();

    let mut models = Vec::new();
    for entry in &m.models {
        let supported_devices = validate_supported_devices(entry)
            .map_err(|e| anyhow::anyhow!("model '{}': {e}", entry.name))?;
        let interval = entry
            .processing_interval_ms
            .map_or_else(|| Duration::from_secs(2), Duration::from_millis);
        models.push(ModelDefinition {
            name: entry.name.clone(),
            provider: entry.provider.clone(),
            source: source.clone(),
            is_multilingual: entry.multilingual,
            estimated_vram_bytes: entry.estimated_vram_bytes,
            processing_interval: interval,
            supported_devices,
            realtime: entry.realtime,
        });
    }

    Ok(DiscoveredBackend {
        dir: dir.to_path_buf(),
        source,
        name: m.backend.name,
        kind: m.backend.kind.to_string(),
        entrypoint: m.backend.entrypoint,
        allowed_hosts: m.network.allowed_hosts,
        secrets: m.secrets,
        options: m.options,
        models,
    })
}

/// Validate a model's declared `supported_devices`. Returns the de-duplicated
/// wire-form device list on success.
///
/// Rules (hard-fail at discovery on any violation):
/// - Field is required: an empty list is rejected. (Unknown device names are
///   already rejected at parse by the `Device` enum.)
/// - The sentinel `"none"` (online/remote model with no local compute) must
///   be the only entry when present — mixing it with local devices is a
///   contradiction.
fn validate_supported_devices(entry: &ModelEntry) -> anyhow::Result<Vec<String>> {
    if entry.supported_devices.is_empty() {
        anyhow::bail!("empty 'supported_devices' — declare at least one device");
    }
    if entry.supported_devices.contains(&Device::None) && entry.supported_devices.len() > 1 {
        anyhow::bail!(
            "'none' (remote/online) must be the only entry in supported_devices when present"
        );
    }
    // Stable de-dup preserving declaration order.
    let mut seen: Vec<Device> = Vec::with_capacity(entry.supported_devices.len());
    for d in &entry.supported_devices {
        if !seen.contains(d) {
            seen.push(*d);
        }
    }
    Ok(seen.iter().map(ToString::to_string).collect())
}

/// Locate the backend and model definition matching `(name, provider, source)`.
///
/// An empty `source` matches the first backend that serves `(name, provider)`.
#[must_use]
pub fn find_model<'a>(
    backends: &'a [DiscoveredBackend],
    name: &str,
    provider: &Provider,
    source: &str,
) -> Option<(&'a DiscoveredBackend, &'a ModelDefinition)> {
    for backend in backends {
        if !source.is_empty() && backend.source != source {
            continue;
        }
        if let Some(def) = backend
            .models
            .iter()
            .find(|d| d.name == name && d.provider == *provider)
        {
            return Some((backend, def));
        }
    }
    None
}

/// Relative install dir (subdir name) of a backend — the stable handle used to
/// persist the active backend (so the selection survives a reinstall).
#[must_use]
pub fn dir_name(backend: &DiscoveredBackend) -> Option<String> {
    backend
        .dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// Flatten all discovered models into `(name, provider, source)` triples.
#[must_use]
pub fn list_models(backends: &[DiscoveredBackend]) -> Vec<(String, Provider, String)> {
    backends
        .iter()
        .flat_map(|b| {
            b.models
                .iter()
                .map(|d| (d.name.clone(), d.provider.clone(), d.source.clone()))
        })
        .collect()
}

/// The default backends search directory: `<data_dir>/super-stt/backends`.
#[must_use]
pub fn default_backends_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".local/share")
        })
        .join("super-stt")
        .join("backends")
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
