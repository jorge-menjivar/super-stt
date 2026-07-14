// SPDX-License-Identifier: GPL-3.0-only
//! Resolve an arbitrary repo URL into an [`IndexBackend`] for the Custom-repo
//! install path.
//!
//! Flow: parse `repo_url` -> ask the forge for the latest release -> download
//! the `backend.toml` release asset -> map each declared binary asset to a
//! release download URL -> build an [`IndexBackend`] whose `manifest` pin
//! points at the `backend.toml` asset with an **empty `sha256`**. The install
//! pipeline treats an empty pin sha as "skip verification" (TLS to the forge is
//! the only integrity guarantee) but still validates the manifest and installs
//! it verbatim — the `unverified_source` warning surfaced to clients (see
//! `docs/protocol/endpoints/v1/registry/install.md`) reflects this.

use super_stt_registry_types::manifest::{Kind, Manifest, ManifestError};
use thiserror::Error;

use crate::registry::index_schema::{
    IndexAsset, IndexAssets, IndexBackend, IndexSubprocessAsset, id_from_source,
};
use super_stt_forge::{ForgeClient, ReleaseAsset, RepoRef};

const MAX_MANIFEST_BYTES: usize = 256 * 1024;

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("repo URL `{0}` is not a <host>/<owner>/<repo> reference")]
    BadRepoUrl(String),
    #[error("forge: {0}")]
    Forge(#[from] super_stt_forge::ForgeError),
    #[error("backend.toml exceeds {MAX_MANIFEST_BYTES} bytes")]
    ManifestTooLarge,
    #[error("backend.toml is not valid UTF-8: {0}")]
    NotUtf8(#[from] std::string::FromUtf8Error),
    #[error("backend.toml invalid: {0}")]
    Manifest(#[from] ManifestError),
    #[error("backend.toml declares kind = `wasm` but no `[assets].wasm` entry")]
    MissingWasmAsset,
    #[error("backend.toml declares kind = `subprocess` but no `[[assets.subprocess]]` entries")]
    MissingSubprocessAssets,
    #[error("release has no asset named `{0}`")]
    AssetMissing(String),
    #[error(
        "backend.toml `source = {declared:?}` is not the repo `{repo}` or namespaced under it (identity spoofing)"
    )]
    SourceSpoof { declared: String, repo: String },
    #[error("backend.toml `{field} = {value:?}` is not a safe relative path component")]
    UnsafeComponent { field: &'static str, value: String },
}

/// Resolve a repo URL into an [`IndexBackend`] suitable for the install
/// pipeline, with empty `sha256` strings (the pipeline skips verification).
///
/// # Errors
/// Returns a [`ResolveError`] when the URL is malformed, the forge is
/// unreachable, the manifest is missing/invalid, or a declared asset isn't
/// present in the release.
pub async fn resolve(
    client: &dyn ForgeClient,
    repo_url: &str,
) -> Result<IndexBackend, ResolveError> {
    let repo = RepoRef::parse(repo_url).map_err(|_| ResolveError::BadRepoUrl(repo_url.into()))?;
    let release = client.latest_release(&repo).await?;

    // The manifest is the `backend.toml` release asset — the exact bytes the
    // daemon installs verbatim. Read and validate those, and pin the asset
    // (empty sha: unverified, TLS-only — there is no index to pre-compute a
    // hash).
    let manifest_asset = release
        .assets
        .iter()
        .find(|a| a.name == "backend.toml")
        .ok_or_else(|| ResolveError::AssetMissing("backend.toml".into()))?;
    if manifest_asset.size > MAX_MANIFEST_BYTES as u64 {
        return Err(ResolveError::ManifestTooLarge);
    }
    let manifest_url = manifest_asset.download_url.clone();
    let manifest_size = manifest_asset.size;
    // The declared `size` above is attacker-controlled metadata, so enforce the
    // real cap during the transfer: `download` streams and aborts the moment the
    // body would exceed it, rather than buffering an unbounded body first.
    let manifest_bytes = client
        .download(&manifest_url, MAX_MANIFEST_BYTES as u64)
        .await
        .map_err(|e| match e {
            super_stt_forge::ForgeError::TooLarge { .. } => ResolveError::ManifestTooLarge,
            e @ super_stt_forge::ForgeError::Http(_) => ResolveError::Forge(e),
        })?;
    let manifest_text = String::from_utf8(manifest_bytes)?;
    // Parse through the canonical manifest so a custom-repo install is validated
    // exactly as the daemon's own discovery will validate it: typed
    // kind/provider/device/accel, required descriptions, and the safe-entrypoint
    // / safe-destination guards all come from the single shared parser.
    let manifest = Manifest::parse(&manifest_text)?;

    // Unlike the registry path, a custom repo's manifest is never run through
    // the indexer's source-vs-repo check. Enforce it here (see
    // `ensure_source_matches_repo`). The entrypoint safety guard is already
    // applied by `Manifest::parse`.
    ensure_source_matches_repo(&manifest.backend.source, &repo)?;

    let assets = synthesize_assets(&manifest, &release.assets)?;

    let id = id_from_source(&manifest.backend.source);
    if !super_stt_shared::registry::is_safe_component(&id) {
        return Err(ResolveError::UnsafeComponent {
            field: "source",
            value: manifest.backend.source,
        });
    }
    let version = manifest.backend.version.trim_start_matches('v').to_string();

    // Shared synthesis: identical field mapping to the registry indexer and the
    // local-dir path (name-fallback labels, "string" option-type default, …).
    Ok(IndexBackend::from_manifest(
        id,
        manifest,
        version,
        release.tag,
        assets,
        Some(IndexAsset {
            url: manifest_url,
            size: manifest_size,
            sha256: String::new(),
        }),
    ))
}

/// The declared `source` must be the repo the user pointed at
/// (`<host>/<owner>/<repo>`) or namespaced under it. A `source` under a
/// *different* repo is identity spoofing — e.g. a malicious repo claiming
/// `source = github.com/jorge-menjivar/super-stt/openai` to overwrite the
/// official backend and make the daemon resolve that source to the attacker's
/// install. Mirrors the indexer's `manifest::validate`.
fn ensure_source_matches_repo(source: &str, repo: &RepoRef) -> Result<(), ResolveError> {
    let canon = repo.canonical();
    let under_repo = source.starts_with(&format!("{canon}/"));
    if source != canon && !under_repo {
        return Err(ResolveError::SourceSpoof {
            declared: source.into(),
            repo: canon,
        });
    }
    Ok(())
}

fn synthesize_assets(
    manifest: &Manifest,
    release_assets: &[ReleaseAsset],
) -> Result<IndexAssets, ResolveError> {
    let find = |name: &str| -> Result<&ReleaseAsset, ResolveError> {
        release_assets
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| ResolveError::AssetMissing(name.into()))
    };

    let mut out = IndexAssets::default();
    match manifest.backend.kind {
        Kind::Wasm => {
            let file = manifest
                .assets
                .wasm
                .as_deref()
                .ok_or(ResolveError::MissingWasmAsset)?;
            let a = find(file)?;
            out.wasm = Some(IndexAsset {
                url: a.download_url.clone(),
                size: a.size,
                sha256: String::new(),
            });
        }
        Kind::Subprocess => {
            if manifest.assets.subprocess.is_empty() {
                return Err(ResolveError::MissingSubprocessAssets);
            }
            for sa in &manifest.assets.subprocess {
                // A variant is one `file` or several concatenated `parts` (the
                // file-xor-parts invariant is guaranteed by `Manifest::parse`).
                // The custom-repo path has no pinned hash (TLS is the
                // guarantee), so each synthesized pin carries an empty sha256.
                let names = sa.release_files();
                if names.is_empty() {
                    return Err(ResolveError::MissingSubprocessAssets);
                }
                let mut pins: Vec<IndexAsset> = Vec::with_capacity(names.len());
                for n in &names {
                    let a = find(n)?;
                    pins.push(IndexAsset {
                        url: a.download_url.clone(),
                        size: a.size,
                        sha256: String::new(),
                    });
                }
                let (url, size, sha256, parts) = if sa.is_multipart() {
                    (None, None, None, pins)
                } else {
                    let p = pins.remove(0);
                    (Some(p.url), Some(p.size), Some(p.sha256), Vec::new())
                };
                out.subprocess.push(IndexSubprocessAsset {
                    target: sa.target.clone(),
                    accel: sa.accel.to_string(),
                    cuda_major: sa.cuda_major,
                    cuda_sm: sa.cuda_sm,
                    cudnn: sa.cudnn,
                    url,
                    size,
                    sha256,
                    parts,
                });
            }
        }
    }
    Ok(out)
}

// The manifest types are the canonical `super_stt_registry_types::manifest`
// set (see the imports): custom_repo parses a remote repo's `backend.toml`
// through the same strict parser the daemon's discovery uses, then projects the
// asset-selection subset onto the loose registry-index shape (`IndexBackend`).

#[cfg(test)]
mod tests {
    use super::*;
    use super_stt_forge::RepoRef;

    #[test]
    fn source_matching_repo_or_namespaced_under_it_is_accepted() {
        let repo = RepoRef::parse("github.com/a/b").unwrap();
        ensure_source_matches_repo("github.com/a/b", &repo).unwrap();
        ensure_source_matches_repo("github.com/a/b/openai", &repo).unwrap();
    }

    #[test]
    fn source_under_a_different_repo_is_rejected_as_spoof() {
        let repo = RepoRef::parse("github.com/a/b").unwrap();
        // A repo at `a/b` claiming an identity owned by `jorge-menjivar/super-stt`.
        let err = ensure_source_matches_repo("github.com/jorge-menjivar/super-stt/openai", &repo)
            .unwrap_err();
        assert!(matches!(err, ResolveError::SourceSpoof { .. }));
        // Prefix-only overlap must not pass (requires a `/` boundary).
        let err = ensure_source_matches_repo("github.com/a/bbb", &repo).unwrap_err();
        assert!(matches!(err, ResolveError::SourceSpoof { .. }));
    }

    #[test]
    fn synthesize_wasm_picks_url_from_release() {
        let manifest_text = r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "v1"
            description = "Test backend."

            [assets]
            wasm = "y.wasm"
        "#;
        let m = Manifest::parse(manifest_text).unwrap();
        let release_assets = vec![ReleaseAsset {
            name: "y.wasm".into(),
            download_url: "https://example/y.wasm".into(),
            size: 42,
        }];
        let assets = synthesize_assets(&m, &release_assets).unwrap();
        let wasm = assets.wasm.as_ref().unwrap();
        assert_eq!(wasm.url, "https://example/y.wasm");
        assert_eq!(wasm.size, 42);
        assert!(wasm.sha256.is_empty());
    }

    #[test]
    fn synthesize_fails_when_release_lacks_declared_asset() {
        let manifest_text = r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "v1"
            description = "Test backend."

            [assets]
            wasm = "missing.wasm"
        "#;
        let m = Manifest::parse(manifest_text).unwrap();
        let err = synthesize_assets(&m, &[]).unwrap_err();
        assert!(matches!(err, ResolveError::AssetMissing(_)));
    }

    #[test]
    fn synthesize_subprocess_maps_each_declared_asset_to_release_url() {
        let manifest_text = r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"
            description = "Test backend."

            [[assets.subprocess]]
            file = "y-cpu.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = "cpu"

            [[assets.subprocess]]
            file = "y-cuda.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = "cuda"
            cuda_major = 12
            cuda_sm = 86
        "#;
        let m = Manifest::parse(manifest_text).unwrap();
        let release_assets = vec![
            ReleaseAsset {
                name: "y-cpu.tar.gz".into(),
                download_url: "https://example/cpu".into(),
                size: 1,
            },
            ReleaseAsset {
                name: "y-cuda.tar.gz".into(),
                download_url: "https://example/cuda".into(),
                size: 2,
            },
        ];
        let assets = synthesize_assets(&m, &release_assets).unwrap();
        assert!(assets.wasm.is_none());
        assert_eq!(assets.subprocess.len(), 2);
        assert_eq!(assets.subprocess[1].cuda_sm, Some(86));
        // Single-file custom-repo synth carries an empty (unverified) pin.
        assert_eq!(assets.subprocess[1].sha256.as_deref(), Some(""));
        assert!(assets.subprocess[1].parts.is_empty());
    }

    #[test]
    fn synthesize_subprocess_maps_parts_to_a_multipart_entry() {
        let manifest_text = r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"
            description = "Test backend."

            [[assets.subprocess]]
            parts = ["y-cuda13.tar.gz.part00", "y-cuda13.tar.gz.part01"]
            target = "x86_64-unknown-linux-gnu"
            accel = "cuda"
            cuda_major = 13
        "#;
        let m = Manifest::parse(manifest_text).unwrap();
        let release_assets = vec![
            ReleaseAsset {
                name: "y-cuda13.tar.gz.part00".into(),
                download_url: "https://example/p0".into(),
                size: 10,
            },
            ReleaseAsset {
                name: "y-cuda13.tar.gz.part01".into(),
                download_url: "https://example/p1".into(),
                size: 20,
            },
        ];
        let assets = synthesize_assets(&m, &release_assets).unwrap();
        assert_eq!(assets.subprocess.len(), 1);
        let a = &assets.subprocess[0];
        // Multi-part: no single-file pin, the parts carry the URLs in order.
        assert!(a.url.is_none());
        assert_eq!(a.parts.len(), 2);
        assert_eq!(a.parts[0].url, "https://example/p0");
        assert_eq!(a.parts[1].size, 20);
        // Unverified custom-repo source → empty per-part pins.
        assert!(a.parts.iter().all(|p| p.sha256.is_empty()));
    }
}
