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

use crate::registry::index_schema::{IndexAssets, IndexBackend, id_from_source};
use crate::stt_models::backends::manifest::Manifest;

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
    #[error(
        "local_path `{0}` has no `{1}`, the entrypoint backend.toml declares (a build tree names the artifact after the crate — stage it under the entrypoint name)"
    )]
    NoEntrypoint(String, String),
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
/// has no `backend.toml`, does not contain the file `[backend].entrypoint`
/// names, or the manifest fails to parse or validate.
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

    // A registry release ships the entrypoint built and named; an import is
    // staged by hand, so nothing else establishes that it is there. Checked
    // here rather than left to the loader: the install would otherwise succeed
    // and the backend fail at model load with a read error naming a path,
    // long after the operator could connect it to what they staged.
    if !local_path.join(&m.backend.entrypoint).is_file() {
        return Err(ResolveError::NoEntrypoint(
            local_path.display().to_string(),
            m.backend.entrypoint.clone(),
        ));
    }

    let id = id_from_source(&m.backend.source);
    if !super_stt_shared::registry::is_safe_component(&id) {
        return Err(ResolveError::UnsafeId(m.backend.source.clone()));
    }
    let version = m.backend.version.trim_start_matches('v').to_string();
    let tag = m.backend.version.clone();

    // Shared synthesis with the registry indexer and Custom-repo path. Local
    // installs have no downloadable assets and no pinned manifest — the files
    // are copied verbatim from the staged dir. Previously this path hand-rolled
    // the mapping and silently dropped the manifest's secrets, options, and
    // license; `from_manifest` now maps them like every other install path.
    Ok(IndexBackend::from_manifest(
        id,
        m,
        version,
        tag,
        IndexAssets::default(),
        None,
    ))
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
description = "Test backend."
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
    fn maps_secrets_and_options_from_manifest() {
        // Regression: the local-dir path used to hardcode `secrets: Vec::new()`
        // / `options: Vec::new()`, so an imported backend lost its declared
        // secrets/options (and their name-fallback labels). It now shares the
        // canonical `from_manifest` synthesis with every other install path.
        let manifest = format!(
            "{MIN_MANIFEST}\n\
             [[secrets]]\n\
             name = \"y_api_key\"\n\
             description = \"Key.\"\n\
             \n\
             [[options]]\n\
             name = \"base_url\"\n\
             description = \"Override.\"\n"
        );
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("backend.toml"), manifest).unwrap();
        fs::write(dir.path().join("y.wasm"), [0u8; 4]).unwrap();
        let entry = resolve(dir.path()).unwrap();
        assert_eq!(entry.secrets.len(), 1);
        assert_eq!(entry.secrets[0].name, "y_api_key");
        assert_eq!(entry.secrets[0].label, "y_api_key"); // falls back to name
        assert_eq!(entry.options.len(), 1);
        assert_eq!(entry.options[0].name, "base_url");
        assert_eq!(entry.options[0].r#type, "string"); // default type
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

    /// The shape that sent an operator here: a source checkout, where the build
    /// tree holds the component under the crate's name rather than the
    /// entrypoint's. Caught at install, while the operator still knows what they
    /// staged, instead of at model load as a read error naming a path.
    #[test]
    fn rejects_dir_without_its_entrypoint() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("backend.toml"), MIN_MANIFEST).unwrap();
        fs::create_dir_all(dir.path().join("target/wasm32-wasip2/release")).unwrap();
        fs::write(
            dir.path()
                .join("target/wasm32-wasip2/release/crate_name.wasm"),
            [0u8; 4],
        )
        .unwrap();
        let err = resolve(dir.path()).unwrap_err();
        assert!(matches!(err, ResolveError::NoEntrypoint(_, ref e) if e == "y.wasm"));
        // The message names the file to stage, since that is the operator's fix.
        assert!(err.to_string().contains("y.wasm"), "{err}");
    }

    /// A directory sharing the entrypoint's name is not an entrypoint. The
    /// loader would fail to read it exactly as if nothing were staged.
    #[test]
    fn rejects_an_entrypoint_that_is_not_a_file() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("backend.toml"), MIN_MANIFEST).unwrap();
        fs::create_dir(dir.path().join("y.wasm")).unwrap();
        let err = resolve(dir.path()).unwrap_err();
        assert!(matches!(err, ResolveError::NoEntrypoint(..)));
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
