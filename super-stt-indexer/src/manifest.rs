// SPDX-License-Identifier: GPL-3.0-only
//! Fetch + registry-policy validation of a backend's `backend.toml` at a tag.
//! The manifest types and parser are canonical in `super-stt-registry-types`.

use semver::Version;
use thiserror::Error;

pub use super_stt_registry_types::manifest::{
    Kind, Manifest, ManifestError as ParseError, SubprocessAsset,
};

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("`backend.version = {0:?}` does not match tag version `{1}`")]
    VersionMismatch(String, Version),
    #[error("`backend.source = {0:?}` does not match registry entry repo `{1}`")]
    SourceMismatch(String, String),
    /// The release manifest's `[backend].id` does not equal the `id` declared
    /// by the `registry.toml` entry that points at this release.
    #[error("manifest declares `id = {0:?}` but the registry entry declares {1:?}")]
    IdMismatch(String, String),
    #[error("`backend.kind = \"wasm\"` requires `[assets.wasm]` but it is missing")]
    MissingWasmAsset,
    #[error("`backend.kind = \"subprocess\"` requires `[[assets.subprocess]]` but list is empty")]
    MissingSubprocessAssets,
    #[error("missing license; declare `[backend].license` (a recognized SPDX id or \"other\")")]
    MissingLicense,
    #[error(
        "license `{0}` is not a recognized open-source license; use a current \
         OSI-approved or FSF Free/Libre SPDX identifier (e.g. Apache-2.0, MIT, \
         GPL-3.0-only) or \"other\""
    )]
    LicenseNotAllowed(String),
    #[error(
        "option `base_url` must not declare a `default`: its value authorizes \
         egress the sandbox would otherwise refuse, so it has to come from the \
         user. Carry the endpoint in the component and leave the option as an \
         override."
    )]
    BaseUrlDefault,
}

pub fn validate(
    m: &Manifest,
    expected_version: &Version,
    expected_source: &str,
    expected_id: Option<&str>,
) -> Result<(), ManifestError> {
    // Parse via the one shared version parser (v-prefix strip + semver) so the
    // manifest check can't drift from the daemon/app/resolve logic (Tier 1 #31).
    let v =
        super_stt_registry_types::version::parse_version(&m.backend.version).ok_or_else(|| {
            ManifestError::VersionMismatch(m.backend.version.clone(), expected_version.clone())
        })?;
    if &v != expected_version {
        return Err(ManifestError::VersionMismatch(
            m.backend.version.clone(),
            expected_version.clone(),
        ));
    }
    // The backend's `source` is its unique identity and must be controlled by
    // whoever controls the release `repo`: either it equals the repo (a
    // single-backend repo) or it is namespaced under it (a monorepo, where
    // several backends share one repo but each needs a distinct source). A
    // source pointing outside the repo is rejected as spoofing.
    let under_repo = m.backend.source.starts_with(&format!("{expected_source}/"));
    if m.backend.source != expected_source && !under_repo {
        return Err(ManifestError::SourceMismatch(
            m.backend.source.clone(),
            expected_source.into(),
        ));
    }
    // An entry that declares an `id` pins the release to it. This is the same
    // class of check as `SourceMismatch`: whoever controls the entry controls
    // which identity the release may claim, so a release cannot rename itself
    // into another backend's install directory.
    if let Some(want) = expected_id
        && m.backend.id.as_deref() != Some(want)
    {
        return Err(ManifestError::IdMismatch(
            m.backend.id.clone().unwrap_or_default(),
            want.to_string(),
        ));
    }
    match m.backend.kind {
        Kind::Wasm => {
            if m.assets.wasm.is_none() {
                return Err(ManifestError::MissingWasmAsset);
            }
        }
        Kind::Subprocess => {
            if m.assets.subprocess.is_empty() {
                return Err(ManifestError::MissingSubprocessAssets);
            }
        }
    }
    // Accel/cuda/rocm/vulkan cross-field validation (`cuda_major` required when
    // `accel` contains `cuda`, and so on) is enforced by `Manifest::parse`
    // itself, which every caller of `validate` has already run successfully —
    // repeating it here would be dead code that can never trigger.
    // A `base_url` value is user intent: the daemon authorizes the host it names
    // for egress with the SSRF guard relaxed. A manifest-supplied one is the
    // backend author's, so a release carrying it does not go in the registry.
    // The daemon is laxer on purpose — it drops the value and loads the backend
    // — because refusing to load punishes the user for the author's mistake;
    // refusing to publish stops it reaching users at all.
    if m.options.iter().any(|o| {
        o.name == super_stt_registry_types::manifest::BASE_URL_OPTION && o.default.is_some()
    }) {
        return Err(ManifestError::BaseUrlDefault);
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
        description = "Test backend."
        license = "Apache-2.0"

        [assets]
        wasm = "y.wasm"
    "#;

    fn with_id(id: &str) -> String {
        VALID.replace("[backend]", &format!("[backend]\n    id = \"{id}\""))
    }

    #[test]
    fn validates_a_correct_wasm_manifest() {
        let m = Manifest::parse(VALID).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y", None).unwrap();
    }

    #[test]
    fn accepts_a_manifest_whose_id_matches_the_entry() {
        let m = Manifest::parse(&with_id("com.example.y")).unwrap();
        validate(
            &m,
            &Version::new(1, 0, 0),
            "github.com/x/y",
            Some("com.example.y"),
        )
        .expect("matching ids validate");
    }

    #[test]
    fn rejects_a_manifest_whose_id_differs_from_the_entry() {
        let m = Manifest::parse(&with_id("com.example.other")).unwrap();
        let err = validate(
            &m,
            &Version::new(1, 0, 0),
            "github.com/x/y",
            Some("com.example.y"),
        )
        .unwrap_err();
        assert!(matches!(err, ManifestError::IdMismatch(_, _)), "{err:?}");
    }

    #[test]
    fn rejects_a_manifest_with_no_id_when_the_entry_declares_one() {
        let m = Manifest::parse(VALID).unwrap();
        let err = validate(
            &m,
            &Version::new(1, 0, 0),
            "github.com/x/y",
            Some("com.example.y"),
        )
        .unwrap_err();
        assert!(matches!(err, ManifestError::IdMismatch(_, _)), "{err:?}");
    }

    /// A grandfathered entry declares no id, so the manifest is not pinned.
    #[test]
    fn accepts_any_id_when_the_entry_declares_none() {
        let m = Manifest::parse(VALID).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y", None).expect("unpinned");
        let m = Manifest::parse(&with_id("com.example.y")).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y", None).expect("unpinned");
    }

    /// The indexer fetches `backend.toml` from a backend's release, so it sees
    /// whatever released backends declare — including `provider`, which this
    /// workspace no longer reads. Accepting it is what keeps already-published
    /// backends in the index; rejecting it would silently drop them.
    ///
    /// The parse-level guarantee is pinned in `super-stt-registry-types`; this
    /// pins that the indexer's own `validate` gate passes it too.
    #[test]
    fn accepts_a_manifest_whose_model_declares_an_unread_provider() {
        let t = format!(
            "{VALID}
            [[models]]
            name = \"m1\"
            provider = \"local_whisper\"
            primary_language = \"en\"
            supported_languages = [\"en\"]
            supported_devices = [\"cpu\"]
            "
        );
        let m = Manifest::parse(&t).expect("a manifest declaring `provider` must still parse");
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y", None)
            .expect("`provider` must not fail indexer validation");
        assert_eq!(m.models.len(), 1);
    }

    #[test]
    fn rejects_version_mismatch() {
        let m = Manifest::parse(VALID).unwrap();
        let err = validate(&m, &Version::new(2, 0, 0), "github.com/x/y", None).unwrap_err();
        assert!(matches!(err, ManifestError::VersionMismatch(_, _)));
    }

    #[test]
    fn rejects_source_mismatch() {
        let m = Manifest::parse(VALID).unwrap();
        let err = validate(&m, &Version::new(1, 0, 0), "github.com/other/repo", None).unwrap_err();
        assert!(matches!(err, ManifestError::SourceMismatch(_, _)));
    }

    #[test]
    fn accepts_monorepo_subpath_source() {
        let t = VALID.replace("github.com/x/y", "github.com/x/y/openai");
        let m = Manifest::parse(&t).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y", None).unwrap();
    }

    #[test]
    fn rejects_source_that_only_shares_a_prefix_segment() {
        let t = VALID.replace("github.com/x/y", "github.com/x/yyy");
        let m = Manifest::parse(&t).unwrap();
        let err = validate(&m, &Version::new(1, 0, 0), "github.com/x/y", None).unwrap_err();
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
            description = "Test backend."
        "#;
        let err: ManifestError = Manifest::parse(t).unwrap_err().into();
        assert!(matches!(
            err,
            ManifestError::Parse(ParseError::UnsafeEntrypoint(_))
        ));
    }

    #[test]
    fn rejects_cuda_without_required_fields() {
        // Accel/cuda cross-field validation is enforced by `Manifest::parse`
        // itself (canonical in `super-stt-registry-types`); this only pins that
        // the guard surfaces as this crate's `ManifestError::Parse`.
        let t = r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"
            description = "Test backend."
            license = "Apache-2.0"

            [[assets.subprocess]]
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = "cuda"
        "#;
        let err: ManifestError = Manifest::parse(t).unwrap_err().into();
        assert!(matches!(
            err,
            ManifestError::Parse(ParseError::CudaMissingMajor { .. })
        ));
    }

    /// A release may declare the `base_url` option, but not a value for it: the
    /// host it names is authorized for egress with the SSRF guard relaxed, which
    /// only the user may ask for. The registry is where that is refused — the
    /// daemon loads such a backend with the value dropped.
    #[test]
    fn rejects_a_base_url_default_but_accepts_the_option() {
        const BASE: &str = r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "v1"
            description = "Test backend."
            license = "Apache-2.0"

            [assets]
            wasm = "y.wasm"

            [[options]]
            name = "base_url"
            description = "Endpoint."
            type = "string"
        "#;
        let m = Manifest::parse(BASE).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y", None).unwrap();

        let m = Manifest::parse(&format!("{BASE}\ndefault = \"https://api.y.com\"\n")).unwrap();
        let err = validate(&m, &Version::new(1, 0, 0), "github.com/x/y", None).unwrap_err();
        assert!(
            matches!(err, ManifestError::BaseUrlDefault),
            "expected BaseUrlDefault, got {err:?}"
        );

        // Every other option keeps its default.
        let m =
            Manifest::parse(&BASE.replace(r#"name = "base_url""#, r#"name = "region""#)).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y", None).unwrap();
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
            description = "Test backend."
            license = "Apache-2.0"

            [[assets.subprocess]]
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = "cuda"
            cuda_major = 13
        "#;
        let m = Manifest::parse(t).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y", None).unwrap();
    }
}
