// SPDX-License-Identifier: GPL-3.0-only
//! Resolve a locally-staged backend directory into an [`IndexBackend`] for
//! the Import-from-dir install path.
//!
//! The operator hands the daemon a directory they already laid out — the
//! daemon parses `<local_path>/backend.toml`, validates it with the
//! workspace's canonical manifest parser, and synthesizes the same
//! [`IndexBackend`] shape the registry / Custom-repo paths produce. The
//! install task then copies the directory into place (see
//! [`crate::registry::install::run_local`]) instead of downloading.

use std::path::Path;

use thiserror::Error;

use crate::registry::index_schema::{IndexAssets, IndexBackend, IndexModel};
use crate::stt_models::backends::manifest::{Device, Manifest};
use super_stt_shared::models::provider::Provider;

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("local_path `{0}` must be an absolute path")]
    NotAbsolute(String),
    #[error("local_path `{0}` does not exist")]
    NotFound(String),
    #[error("local_path `{0}` is not a directory")]
    NotADirectory(String),
    #[error("local_path `{0}` has no backend.toml")]
    NoManifest(String),
    #[error("backend.toml: {0:#}")]
    Manifest(#[from] anyhow::Error),
    #[error("backend.toml `source = {0:?}` yields an unsafe install id")]
    UnsafeId(String),
}

/// Read `<local_path>/backend.toml`, validate it, and synthesize an
/// [`IndexBackend`]. Returns the directory unchanged in
/// `IndexBackend.entrypoint` etc. — the caller is responsible for copying.
///
/// # Errors
/// Returns a [`ResolveError`] when the path is missing, not a directory,
/// has no `backend.toml`, or the manifest fails to parse or validate.
pub fn resolve(local_path: &Path) -> Result<IndexBackend, ResolveError> {
    if !local_path.is_absolute() {
        return Err(ResolveError::NotAbsolute(local_path.display().to_string()));
    }
    if !local_path.exists() {
        return Err(ResolveError::NotFound(local_path.display().to_string()));
    }
    if !local_path.is_dir() {
        return Err(ResolveError::NotADirectory(
            local_path.display().to_string(),
        ));
    }
    if !local_path.join("backend.toml").exists() {
        return Err(ResolveError::NoManifest(local_path.display().to_string()));
    }
    let m = Manifest::load(local_path).map_err(anyhow::Error::from)?;
    crate::stt_models::backends::manifest::validate_runtime(&m)?;

    let online = m
        .models
        .iter()
        .any(|md| matches!(md.provider, Provider::Online(_)));

    let id = id_from_source(&m.backend.source);
    if !super_stt_shared::registry::is_safe_component(&id) {
        return Err(ResolveError::UnsafeId(m.backend.source.clone()));
    }
    let version = m.backend.version.trim_start_matches('v').to_string();
    let tag = m.backend.version.clone();

    Ok(IndexBackend {
        id,
        source: m.backend.source.clone(),
        version,
        tag,
        name: m.backend.name.clone(),
        description: None,
        license: String::new(),
        kind: m.backend.kind.to_string(),
        contract: m.backend.contract.to_string(),
        entrypoint: m.backend.entrypoint.clone(),
        allowed_hosts: m.network.allowed_hosts.clone(),
        online,
        supports_gpu: m.models.iter().any(|md| {
            md.supported_devices
                .iter()
                .any(|d| matches!(d, Device::Cuda | Device::Metal))
        }),
        supports_cpu: m
            .models
            .iter()
            .any(|md| md.supported_devices.contains(&Device::Cpu)),
        models: m
            .models
            .iter()
            .map(|md| IndexModel {
                name: md.name.clone(),
                provider: md.provider.to_string(),
                supported_devices: md
                    .supported_devices
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            })
            .collect(),
        secrets: Vec::new(),
        options: Vec::new(),
        assets: IndexAssets::default(),
        index_stale: None,
    })
}

fn id_from_source(source: &str) -> String {
    source.rsplit('/').next().unwrap_or(source).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const MIN_MANIFEST: &str = r#"
[backend]
source = "github.com/x/y"
name = "Y"
version = "1.2.3"
kind = "wasm"
entrypoint = "y.wasm"
contract = "v1"
"#;

    #[test]
    fn resolves_a_valid_local_dir_into_an_index_backend() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("backend.toml"), MIN_MANIFEST).unwrap();
        fs::write(dir.path().join("y.wasm"), [0u8; 4]).unwrap();
        let entry = resolve(dir.path()).unwrap();
        assert_eq!(entry.id, "y");
        assert_eq!(entry.source, "github.com/x/y");
        assert_eq!(entry.version, "1.2.3");
        assert_eq!(entry.kind, "wasm");
        assert_eq!(entry.entrypoint, "y.wasm");
    }

    #[test]
    fn rejects_non_absolute_path() {
        let err = resolve(Path::new("relative/path")).unwrap_err();
        assert!(matches!(err, ResolveError::NotAbsolute(_)));
    }

    #[test]
    fn rejects_missing_dir() {
        let err = resolve(Path::new("/this/should/not/exist/super_stt_test")).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
    }

    #[test]
    fn rejects_dir_without_manifest() {
        let dir = tempdir().unwrap();
        let err = resolve(dir.path()).unwrap_err();
        assert!(matches!(err, ResolveError::NoManifest(_)));
    }

    #[test]
    fn rejects_source_that_yields_unsafe_id() {
        // `source` ending in an empty segment derives an empty install id.
        let manifest = MIN_MANIFEST.replace("github.com/x/y", "github.com/x/y/");
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("backend.toml"), manifest).unwrap();
        fs::write(dir.path().join("y.wasm"), [0u8; 4]).unwrap();
        let err = resolve(dir.path()).unwrap_err();
        assert!(matches!(err, ResolveError::UnsafeId(_)));
    }
}
