// SPDX-License-Identifier: GPL-3.0-only
//! Fetch + registry-policy validation of a backend's `backend.toml` at a tag.
//! The manifest types and parser are canonical in `super-stt-registry-types`.

use semver::Version;
use thiserror::Error;

pub use super_stt_registry_types::manifest::{
    Accel, Device, Kind, Manifest, ManifestError as ParseError,
};
pub use super_stt_registry_types::provider::Provider;

use crate::github::GitHub;

pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("manifest exceeds {MAX_MANIFEST_BYTES} bytes")]
    TooLarge,
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("`backend.version = {0:?}` does not match tag version `{1}`")]
    VersionMismatch(String, Version),
    #[error("`backend.source = {0:?}` does not match registry entry repo `{1}`")]
    SourceMismatch(String, String),
    #[error("`backend.kind = \"wasm\"` requires `[assets.wasm]` but it is missing")]
    MissingWasmAsset,
    #[error("`backend.kind = \"subprocess\"` requires `[[assets.subprocess]]` but list is empty")]
    MissingSubprocessAssets,
    #[error("subprocess asset `{file}`: `cuda_major` is required when accel = \"cuda\"")]
    CudaMissingMajor { file: String },
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

pub async fn fetch(gh: &GitHub, owner_repo: &str, subdir: Option<&str>, tag: &str) -> Result<Manifest, ManifestError> {
    let path = match subdir {
        Some(sd) => format!("{}/backend.toml", sd.trim_end_matches('/')),
        None => "backend.toml".to_string(),
    };
    let bytes = gh.fetch_file(owner_repo, &path, tag).await?;
    if bytes.len() > MAX_MANIFEST_BYTES { return Err(ManifestError::TooLarge); }
    let text = String::from_utf8(bytes).map_err(|e| anyhow::anyhow!("manifest not UTF-8: {e}"))?;
    Ok(Manifest::parse(&text)?)
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
    match m.backend.kind {
        Kind::Wasm => { if m.assets.wasm.is_none() { return Err(ManifestError::MissingWasmAsset); } }
        Kind::Subprocess => { if m.assets.subprocess.is_empty() { return Err(ManifestError::MissingSubprocessAssets); } }
    }
    for a in &m.assets.subprocess {
        if a.accel == Accel::Cuda {
            // `cuda_sm` stays optional: omitted means the build matches any
            // compute capability (multi-architecture framework builds).
            if a.cuda_major.is_none() {
                return Err(ManifestError::CudaMissingMajor { file: a.file.clone() });
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
        let m = Manifest::parse(VALID).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y").unwrap();
    }

    #[test]
    fn rejects_version_mismatch() {
        let m = Manifest::parse(VALID).unwrap();
        let err = validate(&m, &Version::new(2, 0, 0), "github.com/x/y").unwrap_err();
        assert!(matches!(err, ManifestError::VersionMismatch(_, _)));
    }

    #[test]
    fn rejects_source_mismatch() {
        let m = Manifest::parse(VALID).unwrap();
        let err = validate(&m, &Version::new(1, 0, 0), "github.com/other/repo").unwrap_err();
        assert!(matches!(err, ManifestError::SourceMismatch(_, _)));
    }

    #[test]
    fn accepts_monorepo_subpath_source() {
        let t = VALID.replace("github.com/x/y", "github.com/x/y/openai");
        let m = Manifest::parse(&t).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y").unwrap();
    }

    #[test]
    fn rejects_source_that_only_shares_a_prefix_segment() {
        let t = VALID.replace("github.com/x/y", "github.com/x/yyy");
        let m = Manifest::parse(&t).unwrap();
        let err = validate(&m, &Version::new(1, 0, 0), "github.com/x/y").unwrap_err();
        assert!(matches!(err, ManifestError::SourceMismatch(_, _)));
    }

    #[test]
    fn unsafe_entrypoint_surfaces_as_parse_error() {
        // Exhaustive entrypoint guard cases are tested in the canonical
        // `super-stt-registry-types` crate; this only pins that the guard
        // surfaces as this crate's `ManifestError::Parse`.
        let t = r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "../escape"
            contract = "v1"
        "#;
        let err: ManifestError = Manifest::parse(t).unwrap_err().into();
        assert!(matches!(err, ManifestError::Parse(ParseError::UnsafeEntrypoint(_))));
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
        let m = Manifest::parse(t).unwrap();
        let err = validate(&m, &Version::new(1, 0, 0), "github.com/x/y").unwrap_err();
        assert!(matches!(err, ManifestError::CudaMissingMajor { .. }));
    }

    #[test]
    fn accepts_cuda_with_major_but_no_sm() {
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
            cuda_major = 13
        "#;
        let m = Manifest::parse(t).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y").unwrap();
    }
}
