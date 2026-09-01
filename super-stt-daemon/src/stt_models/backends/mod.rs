// SPDX-License-Identifier: GPL-3.0-only
//! Backend discovery.
//!
//! Models are served by out-of-tree backends installed under a backends
//! directory. Each backend is a subdirectory containing a `backend.toml`
//! manifest (see [`manifest`]) plus its entrypoint (a `.wasm` component or a
//! native binary). This module scans that directory and turns each manifest
//! into a [`DiscoveredBackend`] carrying fully-resolved [`ModelDefinition`]s.

pub(crate) mod base_url;
pub mod manifest;

use std::path::{Path, PathBuf};
use std::time::Duration;

use log::{error, info, warn};

use crate::stt_models::ModelDefinition;

use manifest::{Device, Manifest, ModelEntry, Opt, Secret};

/// A backend discovered on disk, with the models it serves resolved into
/// [`ModelDefinition`]s keyed by `(name, source)`.
#[derive(Clone, Debug)]
pub struct DiscoveredBackend {
    /// Directory holding `backend.toml` and the entrypoint.
    pub dir: PathBuf,
    /// Repo id (`[backend].source`); the `source` of every model it serves.
    pub source: String,
    /// The backend's declared `[backend].id`, when it has one. Used to break a
    /// duplicate-directory tie in favour of the canonical install path.
    pub id: Option<String>,
    /// Human-facing backend name (`[backend].name`).
    pub name: String,
    /// The backend's own version (`[backend].version`) as of the last scan.
    ///
    /// Carried from the manifest so the catalog can report what is installed
    /// without consulting the registry — which knows only what a release
    /// offers, and nothing at all about a locally imported backend. Callers
    /// reporting a version to a user should prefer [`installed_version`], which
    /// re-reads the manifest; this is the value the running daemon loaded from,
    /// and stands as the fallback when that read fails.
    pub version: String,
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

/// Read a backend's version from the `backend.toml` in `dir`, or `None` when
/// there is no readable manifest there.
///
/// Read per call rather than taken from discovery, because the two answer
/// different questions: [`DiscoveredBackend::version`] is what the daemon
/// loaded from and only moves when it rescans, while this is what is on disk
/// now. Anything reporting a version to a user wants the latter — a backend
/// changed outside the daemon (a hand-edited manifest, an install from another
/// tool) would otherwise read as its pre-change self until the next rescan.
///
/// The same read backs `GET /backends` and the `installed_version` on
/// `GET /registry/backends`, so the version a client sees and the one an update
/// is judged against cannot disagree.
#[must_use]
pub fn installed_version(dir: &Path) -> Option<String> {
    manifest::Manifest::load(dir)
        .ok()
        .map(|m| m.backend.version)
}

/// Scan `backends_dir` for installed backends. Subdirectories without a
/// readable, parseable `backend.toml` are skipped with a warning.
///
/// A pure scan: it selects the backend that serves each `source` and returns
/// the duplicates it supersedes alongside it, but performs no filesystem
/// mutation. Removing a loser is `registry::reconcile`'s job — reading a
/// directory and deleting one are different responsibilities.
#[must_use]
pub fn discover(backends_dir: &Path) -> (Vec<DiscoveredBackend>, Vec<DiscoveredBackend>) {
    let entries = match std::fs::read_dir(backends_dir) {
        Ok(e) => e,
        Err(e) => {
            info!(
                "Backends directory {} not readable ({e}); no backends discovered",
                backends_dir.display()
            );
            return (Vec::new(), Vec::new());
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

/// Split discovered backends into the one that serves each `source` and the
/// duplicates it supersedes.
///
/// `source` is the identity the daemon resolves models and the active backend
/// by, so two directories claiming one source is ambiguous — and worse, the
/// loser strands an orphaned `models/` subtree that nothing will ever load.
/// The winner is chosen deterministically:
///
/// 1. highest `version` — a backend updated in place before migration existed
///    is the newer install, whatever its directory is called;
/// 2. the directory named after the backend's `id`, the canonical location;
/// 3. the lexicographically first directory name, so the result is stable.
///
/// Selection only. Removing a loser is `registry::reconcile`'s job — reading a
/// directory and deleting one are different responsibilities.
fn dedup_sources(
    backends: Vec<DiscoveredBackend>,
) -> (Vec<DiscoveredBackend>, Vec<DiscoveredBackend>) {
    use std::collections::HashMap;

    let mut groups: HashMap<String, Vec<DiscoveredBackend>> = HashMap::new();
    for b in backends {
        groups.entry(b.source.clone()).or_default().push(b);
    }

    let mut winners = Vec::new();
    let mut losers = Vec::new();
    for (source, mut group) in groups {
        group.sort_by(|a, b| {
            let av = super_stt_registry_types::version::parse_version(&a.version);
            let bv = super_stt_registry_types::version::parse_version(&b.version);
            bv.cmp(&av)
                .then_with(|| is_id_named(b).cmp(&is_id_named(a)))
                .then_with(|| a.dir.cmp(&b.dir))
        });
        let mut it = group.into_iter();
        let winner = it.next().expect("a group is never empty");
        for dup in it {
            error!(
                "Backend '{}' at {} duplicates source '{source}', already served from {}; \
                 it will be reconciled and removed.",
                dup.name,
                dup.dir.display(),
                winner.dir.display()
            );
            losers.push(dup);
        }
        winners.push(winner);
    }
    winners.sort_by(|a, b| a.dir.cmp(&b.dir));
    (winners, losers)
}

/// Whether a backend sits in the directory its own `id` names.
fn is_id_named(b: &DiscoveredBackend) -> bool {
    match (&b.id, b.dir.file_name().and_then(|n| n.to_str())) {
        (Some(id), Some(name)) => id == name,
        _ => false,
    }
}

/// Parse one backend directory into a [`DiscoveredBackend`].
///
/// Per-model errors (missing/invalid `supported_devices`) are fatal for the
/// *whole* backend: discovery skips it rather than expose a half-defined
/// model. Unknown device names are rejected at parse time by the typed
/// manifest; this function enforces the cross-field device rules via
/// [`validate_supported_devices`].
fn load_backend(dir: &Path) -> anyhow::Result<DiscoveredBackend> {
    let mut m = Manifest::load(dir)?;
    manifest::validate_runtime(&m)?;
    // A `base_url` value authorizes egress the sandbox would otherwise refuse,
    // so only the user may supply one. A manifest that declares a default is
    // wrong, but refusing to load the whole backend would punish the user for
    // the author's mistake — and leave them with a backend that silently
    // vanished. Drop the value, keep the option, say so. The registry indexer
    // refuses such a release outright, so this is the sideloaded/local case.
    for opt in &mut m.options {
        if opt.name == base_url::OPTION_NAME && opt.default.take().is_some() {
            warn!(
                "Backend {}: ignoring the `base_url` default declared in {}; \
                 only a value the user sets authorizes egress",
                m.backend.source,
                dir.display()
            );
        }
    }
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
            source: source.clone(),
            is_multilingual: entry.multilingual,
            primary_language: entry.primary_language.clone(),
            supported_languages: entry.supported_languages.clone(),
            estimated_vram_bytes: entry.estimated_vram_bytes,
            processing_interval: interval,
            supported_devices,
            realtime: entry.realtime,
            role: entry.role,
            provider: entry.provider.clone(),
        });
    }

    Ok(DiscoveredBackend {
        dir: dir.to_path_buf(),
        source,
        id: m.backend.id.clone(),
        name: m.backend.name,
        version: m.backend.version,
        kind: m.backend.kind.to_string(),
        entrypoint: m.backend.entrypoint,
        allowed_hosts: m.network.allowed_hosts,
        secrets: m.secrets,
        options: m.options,
        models,
    })
}

/// Validate a model's declared `supported_devices`. Returns the de-duplicated
/// device list on success.
///
/// Rules (hard-fail at discovery on any violation):
/// - Field is required: an empty list is rejected. (Unknown device names are
///   already rejected at parse by the `Device` enum.)
/// - The sentinel [`Device::None`] (online/remote model with no local compute)
///   must be the only entry when present — mixing it with local devices is a
///   contradiction.
fn validate_supported_devices(entry: &ModelEntry) -> anyhow::Result<Vec<Device>> {
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
    Ok(seen)
}

/// Locate the backend and model definition matching `(name, source)`.
///
/// `source` must be concrete. It deliberately has no "any backend" form: two
/// backends may serve the same model `name` (see
/// `docs/protocol/backend/contract.md`), so a name-only lookup would resolve
/// by scan order — which comes from `read_dir` and can differ between runs —
/// and `handle_set_model_impl` then *persists* the backend it picked. Callers
/// that accept an omitted `source` on the wire resolve it against the active
/// backend first (`SuperSTTDaemon::active_backend_source`); an empty `source`
/// here matches nothing.
#[must_use]
pub fn find_model<'a>(
    backends: &'a [DiscoveredBackend],
    name: &str,
    source: &str,
) -> Option<(&'a DiscoveredBackend, &'a ModelDefinition)> {
    backends
        .iter()
        .find(|b| b.source == source)
        .and_then(|b| b.models.iter().find(|d| d.name == name).map(|d| (b, d)))
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

/// Flatten all discovered models into `(name, source)` pairs.
#[must_use]
pub fn list_models(backends: &[DiscoveredBackend]) -> Vec<(String, String)> {
    backends
        .iter()
        .flat_map(|b| b.models.iter().map(|d| (d.name.clone(), d.source.clone())))
        .collect()
}

/// The default backends search directory: `<data_dir>/super-stt/backends`.
#[must_use]
pub fn default_backends_dir() -> PathBuf {
    super_stt_shared::paths::data_dir().join("backends")
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
