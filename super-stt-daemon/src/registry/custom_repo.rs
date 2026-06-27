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

use std::str::FromStr;

use serde::Deserialize;
use super_stt_registry_types::manifest::Device;
use thiserror::Error;

use crate::registry::index_schema::{
    IndexAsset, IndexAssets, IndexBackend, IndexModel, IndexOption, IndexSecret,
    IndexSubprocessAsset,
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
    #[error("backend.toml parse: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("backend.toml declares kind = `wasm` but no `[assets].wasm` entry")]
    MissingWasmAsset,
    #[error("backend.toml declares kind = `subprocess` but no `[[assets.subprocess]]` entries")]
    MissingSubprocessAssets,
    #[error("backend.toml declares unknown kind `{0}` (expected `wasm` or `subprocess`)")]
    UnknownKind(String),
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
    let manifest_bytes = client.download(&manifest_url).await?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ResolveError::ManifestTooLarge);
    }
    let manifest_text = String::from_utf8(manifest_bytes)?;
    let manifest: Manifest = toml::from_str(&manifest_text)?;

    // Unlike the registry path, a custom repo's manifest is never run through
    // the indexer's source-vs-repo check. Enforce it here (see
    // `ensure_source_matches_repo`).
    ensure_source_matches_repo(&manifest.backend.source, &repo)?;
    // `entrypoint` is joined onto the install dir (it may be nested, e.g.
    // `bin/launcher`), and `id` becomes that dir's name (a single component).
    if !super_stt_shared::registry::is_safe_relative_path(&manifest.backend.entrypoint) {
        return Err(ResolveError::UnsafeComponent {
            field: "entrypoint",
            value: manifest.backend.entrypoint,
        });
    }

    let assets = synthesize_assets(&manifest, &release.assets)?;

    let ModelSupport {
        online,
        supports_gpu,
        supports_cpu,
    } = classify_models(&manifest.models);

    let id = id_from_source(&manifest.backend.source);
    if !super_stt_shared::registry::is_safe_component(&id) {
        return Err(ResolveError::UnsafeComponent {
            field: "source",
            value: manifest.backend.source,
        });
    }

    Ok(IndexBackend {
        id,
        source: manifest.backend.source,
        version: manifest.backend.version.trim_start_matches('v').to_string(),
        tag: release.tag,
        name: manifest.backend.name,
        description: Some(manifest.backend.description),
        license: manifest.backend.license.unwrap_or_default(),
        kind: manifest.backend.kind,
        contract: manifest.backend.contract,
        entrypoint: manifest.backend.entrypoint,
        allowed_hosts: manifest.network.allowed_hosts,
        online,
        supports_gpu,
        supports_cpu,
        models: manifest
            .models
            .into_iter()
            .map(|m| IndexModel {
                name: m.name,
                provider: m.provider,
                supported_devices: m.supported_devices,
            })
            .collect(),
        secrets: manifest
            .secrets
            .into_iter()
            .map(|s| IndexSecret {
                name: s.name,
                label: s.label,
                required: s.required,
            })
            .collect(),
        options: manifest
            .options
            .into_iter()
            .map(|o| IndexOption {
                name: o.name,
                label: o.label,
                r#type: o.r#type,
                default: o.default,
            })
            .collect(),
        assets,
        index_stale: None,
        manifest: Some(IndexAsset {
            url: manifest_url,
            size: manifest_size,
            sha256: String::new(),
        }),
    })
}

/// `<host>/<owner>/<repo>` -> `<repo>`. Used as the install-dir name. Custom-
/// repo installs collide-by-name with registry installs of the same repo; that
/// is intentional — installing the same backend twice writes the same dir.
fn id_from_source(source: &str) -> String {
    source.rsplit('/').next().unwrap_or(source).to_string()
}

/// Index-level capability flags derived from a manifest's declared models.
struct ModelSupport {
    online: bool,
    supports_gpu: bool,
    supports_cpu: bool,
}

/// Classify a manifest's models through the canonical [`Device`] type (and the
/// `none` device sentinel for online/remote models) so the custom-repo path
/// agrees with the registry indexer on what counts as online / GPU / CPU.
/// Device strings that aren't canonical simply don't match — the lenient parser
/// still records them in the index, and the canonical layer rejects them
/// downstream.
fn classify_models(models: &[Model]) -> ModelSupport {
    let online = models
        .iter()
        .any(|m| m.supported_devices.iter().any(|d| d == "none"));
    let any_device = |pred: fn(Device) -> bool| {
        models.iter().any(|m| {
            m.supported_devices
                .iter()
                .any(|d| Device::from_str(d).is_ok_and(pred))
        })
    };
    ModelSupport {
        online,
        supports_gpu: any_device(|d| matches!(d, Device::Cuda | Device::Metal)),
        supports_cpu: any_device(|d| matches!(d, Device::Cpu)),
    }
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
    match manifest.backend.kind.as_str() {
        "wasm" => {
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
        "subprocess" => {
            if manifest.assets.subprocess.is_empty() {
                return Err(ResolveError::MissingSubprocessAssets);
            }
            for sa in &manifest.assets.subprocess {
                // A variant is one `file` or several concatenated `parts`. The
                // custom-repo path has no pinned hash (TLS is the guarantee), so
                // each synthesized pin carries an empty sha256.
                let names: Vec<&str> = match &sa.file {
                    Some(f) => vec![f.as_str()],
                    None => sa.parts.iter().map(String::as_str).collect(),
                };
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
                let (url, size, sha256, parts) = if sa.file.is_none() {
                    (None, None, None, pins)
                } else {
                    let p = pins.remove(0);
                    (Some(p.url), Some(p.size), Some(p.sha256), Vec::new())
                };
                out.subprocess.push(IndexSubprocessAsset {
                    target: sa.target.clone(),
                    accel: sa.accel.clone(),
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
        other => return Err(ResolveError::UnknownKind(other.into())),
    }
    Ok(out)
}

// ---------- Manifest types (local to this module) ----------
//
// custom_repo vets a remote repository's manifest before install; it needs
// only the asset-selection subset of the full manifest schema.  A minimal,
// lenient local parser is kept here deliberately rather than parsing the whole
// thing through the canonical super-stt-registry-types, which owns the
// installed-backend shape.  The provider/device fields stay `String` here, but
// their *classification* (online / GPU / CPU) runs through the canonical
// `Provider`/`Device` types — see `classify_models` — so this path agrees with
// the registry indexer instead of using ad-hoc string lists.

#[derive(Debug, Deserialize)]
struct Manifest {
    backend: BackendMeta,
    #[serde(default)]
    network: Network,
    #[serde(default)]
    models: Vec<Model>,
    #[serde(default)]
    secrets: Vec<Secret>,
    #[serde(default)]
    options: Vec<Opt>,
    assets: Assets,
}

#[derive(Debug, Deserialize)]
struct BackendMeta {
    source: String,
    name: String,
    version: String,
    kind: String,
    entrypoint: String,
    contract: String,
    description: String,
    #[serde(default)]
    license: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Network {
    #[serde(default)]
    allowed_hosts: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Model {
    name: String,
    provider: String,
    #[serde(default)]
    supported_devices: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Secret {
    name: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    required: bool,
}

#[derive(Debug, Deserialize)]
struct Opt {
    name: String,
    #[serde(default)]
    label: String,
    #[serde(rename = "type", default)]
    r#type: String,
    #[serde(default)]
    default: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Assets {
    #[serde(default)]
    wasm: Option<String>,
    #[serde(default)]
    subprocess: Vec<SubprocessAsset>,
}

#[derive(Debug, Deserialize)]
struct SubprocessAsset {
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    parts: Vec<String>,
    target: String,
    accel: String,
    #[serde(default)]
    cuda_major: Option<u32>,
    #[serde(default)]
    cuda_sm: Option<u32>,
    #[serde(default)]
    cudnn: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use super_stt_forge::RepoRef;

    fn model(name: &str, provider: &str, devices: &[&str]) -> Model {
        Model {
            name: name.into(),
            provider: provider.into(),
            supported_devices: devices.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn classify_marks_online_from_none_device_not_provider() {
        // Online-ness is derived from the `none` device sentinel, not the
        // (now free-form) provider string.
        assert!(classify_models(&[model("m", "openai", &["none"])]).online);
        assert!(classify_models(&[model("m", "mistral", &["none"])]).online);
        assert!(classify_models(&[model("m", "deepgram", &["none"])]).online);
        // A made-up provider is still online if it declares the `none` device.
        assert!(classify_models(&[model("m", "anthropic", &["none"])]).online);
        assert!(classify_models(&[model("m", "totally_bogus", &["none"])]).online);
        // No `none` device → not online, regardless of provider.
        assert!(!classify_models(&[model("m", "openai", &["cpu"])]).online);
        assert!(!classify_models(&[model("m", "local_whisper", &["cpu"])]).online);
        assert!(!classify_models(&[model("m", "openai", &[])]).online);
    }

    #[test]
    fn classify_marks_gpu_for_cuda_or_metal_not_rocm() {
        assert!(classify_models(&[model("m", "local_whisper", &["cuda"])]).supports_gpu);
        assert!(classify_models(&[model("m", "local_whisper", &["metal"])]).supports_gpu);
        // `rocm` is an Accel build axis, never a model Device — not GPU here.
        assert!(!classify_models(&[model("m", "local_whisper", &["rocm"])]).supports_gpu);
        assert!(!classify_models(&[model("m", "local_whisper", &["cpu"])]).supports_gpu);
    }

    #[test]
    fn classify_marks_cpu_only_for_cpu_device() {
        assert!(classify_models(&[model("m", "local_whisper", &["cpu"])]).supports_cpu);
        assert!(!classify_models(&[model("m", "openai", &["none"])]).supports_cpu);
        assert!(!classify_models(&[model("m", "local_whisper", &["cuda"])]).supports_cpu);
    }

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
        let m: Manifest = toml::from_str(manifest_text).unwrap();
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
        let m: Manifest = toml::from_str(manifest_text).unwrap();
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
        let m: Manifest = toml::from_str(manifest_text).unwrap();
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
        let m: Manifest = toml::from_str(manifest_text).unwrap();
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
