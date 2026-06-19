// SPDX-License-Identifier: GPL-3.0-only

//! Bundled catalog of the official Super STT backends.
//!
//! The app embeds the official `backend.toml` manifests from the repo's
//! `backends/` directory at build time and exposes each backend's
//! `allowed_hosts` by `source` via [`by_source`]. This lets the "Online
//! model" badge show where a backend's audio would go even before it is
//! installed, since `GET /backends` only reports backends already on disk.
//! Each manifest is the single source of truth for its backend (see
//! `docs/protocol/backend/config.md`).

use std::sync::LazyLock;

/// One backend known to the app, parsed from its bundled `backend.toml`.
#[derive(Clone, Debug)]
pub struct CatalogBackend {
    /// Canonical repo id, e.g. `github.com/super-stt/openai`. Matches the
    /// `source` reported by `GET /backends` for installed backends.
    pub source: String,
    /// Hosts the backend may reach (`[network] allowed_hosts`). Shown in the
    /// "Online model" badge so the user sees where their audio would go.
    pub allowed_hosts: Vec<String>,
}

/// The embedded manifests. Empty now that every official backend lives in its
/// own repo; add a line here if a backend is ever bundled in-tree again.
const MANIFESTS: &[&str] = &[];

/// Subset of `backend.toml` the catalog needs; unknown fields are ignored.
#[derive(serde::Deserialize)]
struct Manifest {
    backend: BackendMeta,
    #[serde(default)]
    network: Network,
}

#[derive(serde::Deserialize)]
struct BackendMeta {
    source: String,
}

#[derive(serde::Deserialize, Default)]
struct Network {
    #[serde(default)]
    allowed_hosts: Vec<String>,
}

/// Deserialize one bundled manifest into a [`CatalogBackend`], or `None`
/// on parse error.
fn parse(manifest: &str) -> Option<CatalogBackend> {
    let m: Manifest = toml::from_str(manifest).ok()?;
    Some(CatalogBackend {
        source: m.backend.source,
        allowed_hosts: m.network.allowed_hosts,
    })
}

static CATALOG: LazyLock<Vec<CatalogBackend>> =
    LazyLock::new(|| MANIFESTS.iter().filter_map(|m| parse(m)).collect());

/// Look up a backend's catalog entry by its `source` (repo id), if known.
pub fn by_source(source: &str) -> Option<&'static CatalogBackend> {
    CATALOG.iter().find(|b| b.source == source)
}
