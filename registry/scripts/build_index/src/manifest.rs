// SPDX-License-Identifier: GPL-3.0-only
//! Fetch + structurally validate a backend's `backend.toml` at a tag.

use semver::Version;
use serde::Deserialize;
use thiserror::Error;

use crate::github::GitHub;

pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub backend: BackendMeta,
    #[serde(default)]
    pub network: Network,
    #[serde(default)]
    pub models: Vec<Model>,
    #[serde(default)]
    pub secrets: Vec<Secret>,
    #[serde(default)]
    pub options: Vec<Option_>,
    pub assets: Assets,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackendMeta {
    pub source: String,
    pub name: String,
    pub version: String,
    pub kind: String,            // "wasm" | "subprocess"
    pub entrypoint: String,
    pub contract: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Network {
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Model {
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub supported_devices: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Secret {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename = "Option")]
pub struct Option_ {
    pub name: String,
    pub label: String,
    pub r#type: String,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Assets {
    #[serde(default)]
    pub wasm: Option<String>,
    #[serde(default)]
    pub subprocess: Vec<SubprocessAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubprocessAsset {
    pub file: String,
    pub target: String,
    pub accel: String,
    #[serde(default)]
    pub cuda_major: Option<u32>,
    #[serde(default)]
    pub cuda_sm: Option<u32>,
    #[serde(default)]
    pub cudnn: bool,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("manifest exceeds {MAX_MANIFEST_BYTES} bytes")]
    TooLarge,
    #[error("TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("`backend.version = {0:?}` does not match tag version `{1}`")]
    VersionMismatch(String, Version),
    #[error("`backend.source = {0:?}` does not match registry entry repo `{1}`")]
    SourceMismatch(String, String),
    #[error("`backend.entrypoint = {0:?}` is not a safe relative path")]
    UnsafeEntrypoint(String),
    #[error("`backend.kind = {0:?}` requires `[assets.wasm]` but it is missing")]
    MissingWasmAsset(String),
    #[error("`backend.kind = {0:?}` requires `[[assets.subprocess]]` but list is empty")]
    MissingSubprocessAssets(String),
    #[error("backend `kind` must be `wasm` or `subprocess` (got {0:?})")]
    UnknownKind(String),
    #[error("subprocess asset `{file}`: `accel = {accel:?}` is not allowed")]
    UnknownAccel { file: String, accel: String },
    #[error("subprocess asset `{file}`: cuda_major/cuda_sm required when accel = \"cuda\"")]
    CudaMissingFields { file: String },
    #[error("subprocess asset `{file}`: cuda_major/cuda_sm forbidden when accel != \"cuda\"")]
    CudaForbiddenFields { file: String },
    #[error("subprocess asset `{file}`: `cudnn = true` requires `accel = \"cuda\"`")]
    CudnnRequiresCuda { file: String },
    #[error("missing license; declare `[backend].license`")]
    MissingLicense,
    #[error("license `{0}` is not on the allowlist")]
    LicenseNotAllowed(String),
    #[error(transparent)]
    Http(#[from] anyhow::Error),
}

const ALLOWED_ACCEL: &[&str] = &["cpu", "cuda", "metal", "rocm", "vulkan"];

/// A backend `entrypoint` is a relative path the daemon joins onto the install
/// dir; it may be nested (e.g. `bin/launcher`) but must not escape it. Reject
/// empty, absolute, any empty / `.` / `..` component, backslash, and NUL.
/// Mirrors `super_stt_shared::registry::is_safe_relative_path` (the indexer is
/// a standalone crate and cannot depend on the daemon's crates).
fn is_safe_relative_path(s: &str) -> bool {
    if s.is_empty() || s.starts_with('/') || s.contains('\\') || s.contains('\0') {
        return false;
    }
    let mut saw = false;
    for c in s.split('/') {
        if c.is_empty() || c == "." || c == ".." {
            return false;
        }
        saw = true;
    }
    saw
}

pub async fn fetch(gh: &GitHub, owner_repo: &str, subdir: Option<&str>, tag: &str) -> Result<Manifest, ManifestError> {
    let path = match subdir {
        Some(sd) => format!("{}/backend.toml", sd.trim_end_matches('/')),
        None => "backend.toml".to_string(),
    };
    let bytes = gh.fetch_file(owner_repo, &path, tag).await?;
    if bytes.len() > MAX_MANIFEST_BYTES { return Err(ManifestError::TooLarge); }
    let text = String::from_utf8(bytes).map_err(|e| anyhow::anyhow!("manifest not UTF-8: {e}"))?;
    let m: Manifest = toml::from_str(&text)?;
    Ok(m)
}

pub fn validate(m: &Manifest, expected_version: &Version, expected_source: &str) -> Result<(), ManifestError> {
    let v = Version::parse(m.backend.version.trim_start_matches('v'))
        .map_err(|_| ManifestError::VersionMismatch(m.backend.version.clone(), expected_version.clone()))?;
    if &v != expected_version {
        return Err(ManifestError::VersionMismatch(m.backend.version.clone(), expected_version.clone()));
    }
    // The backend's `source` is its unique identity and must be controlled by
    // whoever controls the release `repo`: either it equals the repo (a
    // single-backend repo) or it is namespaced under it (a monorepo, where
    // several backends share one repo but each needs a distinct source). A
    // source pointing outside the repo is rejected as spoofing.
    let under_repo = m.backend.source.starts_with(&format!("{expected_source}/"));
    if m.backend.source != expected_source && !under_repo {
        return Err(ManifestError::SourceMismatch(m.backend.source.clone(), expected_source.into()));
    }
    if !is_safe_relative_path(&m.backend.entrypoint) {
        return Err(ManifestError::UnsafeEntrypoint(m.backend.entrypoint.clone()));
    }
    match m.backend.kind.as_str() {
        "wasm" => { if m.assets.wasm.is_none() { return Err(ManifestError::MissingWasmAsset(m.backend.kind.clone())); } }
        "subprocess" => { if m.assets.subprocess.is_empty() { return Err(ManifestError::MissingSubprocessAssets(m.backend.kind.clone())); } }
        other => return Err(ManifestError::UnknownKind(other.into())),
    }
    for a in &m.assets.subprocess {
        if !ALLOWED_ACCEL.contains(&a.accel.as_str()) {
            return Err(ManifestError::UnknownAccel { file: a.file.clone(), accel: a.accel.clone() });
        }
        if a.accel == "cuda" {
            if a.cuda_major.is_none() || a.cuda_sm.is_none() {
                return Err(ManifestError::CudaMissingFields { file: a.file.clone() });
            }
        } else {
            if a.cuda_major.is_some() || a.cuda_sm.is_some() {
                return Err(ManifestError::CudaForbiddenFields { file: a.file.clone() });
            }
            if a.cudnn {
                return Err(ManifestError::CudnnRequiresCuda { file: a.file.clone() });
            }
        }
    }
    crate::license::check(m.backend.license.as_deref())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
        [backend]
        source = "github.com/x/y"
        name = "Y"
        version = "1.0.0"
        kind = "wasm"
        entrypoint = "y.wasm"
        contract = "v1"
        license = "Apache-2.0"

        [assets]
        wasm = "y.wasm"
    "#;

    #[test]
    fn validates_a_correct_wasm_manifest() {
        let m: Manifest = toml::from_str(VALID).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y").unwrap();
    }

    #[test]
    fn rejects_version_mismatch() {
        let m: Manifest = toml::from_str(VALID).unwrap();
        let err = validate(&m, &Version::new(2, 0, 0), "github.com/x/y").unwrap_err();
        assert!(matches!(err, ManifestError::VersionMismatch(_, _)));
    }

    #[test]
    fn rejects_source_mismatch() {
        let m: Manifest = toml::from_str(VALID).unwrap();
        let err = validate(&m, &Version::new(1, 0, 0), "github.com/other/repo").unwrap_err();
        assert!(matches!(err, ManifestError::SourceMismatch(_, _)));
    }

    #[test]
    fn accepts_monorepo_subpath_source() {
        // A backend living in a shared repo declares a source namespaced under
        // that repo so its identity stays unique. The repo here is the prefix.
        let t = VALID.replace("github.com/x/y", "github.com/x/y/openai");
        let m: Manifest = toml::from_str(&t).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y").unwrap();
    }

    #[test]
    fn rejects_source_that_only_shares_a_prefix_segment() {
        // `github.com/x/yyy` starts with `github.com/x/y` textually but is a
        // different repo — must NOT pass (we require a `/` boundary).
        let t = VALID.replace("github.com/x/y", "github.com/x/yyy");
        let m: Manifest = toml::from_str(&t).unwrap();
        let err = validate(&m, &Version::new(1, 0, 0), "github.com/x/y").unwrap_err();
        assert!(matches!(err, ManifestError::SourceMismatch(_, _)));
    }

    #[test]
    fn rejects_unsafe_entrypoint() {
        for bad in ["../escape", "/usr/bin/python3", "..", "bin/../../escape", "a//b", "bin/"] {
            let t = VALID.replace(r#"entrypoint = "y.wasm""#, &format!("entrypoint = \"{bad}\""));
            let m: Manifest = toml::from_str(&t).unwrap();
            let err = validate(&m, &Version::new(1, 0, 0), "github.com/x/y").unwrap_err();
            assert!(matches!(err, ManifestError::UnsafeEntrypoint(_)), "entrypoint {bad:?} should be rejected");
        }
    }

    #[test]
    fn rejects_cuda_without_required_fields() {
        let t = r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"
            license = "Apache-2.0"

            [[assets.subprocess]]
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = "cuda"
        "#;
        let m: Manifest = toml::from_str(t).unwrap();
        let err = validate(&m, &Version::new(1, 0, 0), "github.com/x/y").unwrap_err();
        assert!(matches!(err, ManifestError::CudaMissingFields { .. }));
    }
}
