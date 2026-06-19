// SPDX-License-Identifier: GPL-3.0-only
//! `super-stt-indexer` — top-level orchestration.

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use log::{error, info, warn};

use super_stt_forge::{ForgeClient, RepoRef};

use crate::manifest::Device;

mod assets;
mod carryforward;
mod index_json;
mod license;
mod local;
mod manifest;
mod registry_toml;
mod resolve;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Build the published index from `registry.toml` + GitHub releases.
    Build(BuildArgs),
    /// Build a local index from staged backends — offline, no GitHub. For
    /// testing the daemon's download/install pipeline against a localhost
    /// static server (see `just serve-test-registry`).
    Local(local::LocalArgs),
}

#[derive(clap::Args, Debug)]
struct BuildArgs {
    /// Path to `registry.toml` to read.
    #[arg(long, default_value = "registry/registry.toml")]
    registry: PathBuf,
    /// Path to the previously-published `index.json` (for carry-forward). If
    /// missing, falls through cleanly — bootstrap mode.
    #[arg(long)]
    prior_index: Option<PathBuf>,
    /// Where to write the new `index.json`.
    #[arg(long, default_value = "index.json")]
    out: PathBuf,
}

pub struct BuildFailure {
    pub error: String,
    pub attempted_version: Option<String>,
    pub attempted_tag: Option<String>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    // Workspace reqwest uses rustls without a bundled provider; install one.
    let _ = rustls::crypto::ring::default_provider().install_default();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    match Args::parse().command {
        Command::Build(args) => run_build(args).await,
        Command::Local(args) => local::run(&args),
    }
}

/// Build the published index from `registry.toml` + GitHub releases.
async fn run_build(args: BuildArgs) -> anyhow::Result<()> {
    let registry_text = std::fs::read_to_string(&args.registry)
        .with_context(|| format!("reading {}", args.registry.display()))?;
    let registry = registry_toml::Registry::parse(&registry_text)?;

    let prior = match args.prior_index.as_ref() {
        Some(p) if p.exists() => {
            let text = std::fs::read_to_string(p)?;
            Some(serde_json::from_str::<index_json::Index>(&text)?)
        }
        _ => None,
    };

    let http = reqwest::Client::new();
    let now_iso = chrono_now_iso();

    let mut out_backends: Vec<index_json::IndexBackend> = Vec::new();

    for (id, entry) in &registry.0 {
        if entry.removed {
            info!("skip `{id}` — removed");
            continue;
        }
        let client = super_stt_forge::client(entry.forge);
        let repo = RepoRef::parse(&entry.repo)?;
        match build_entry(client.as_ref(), &http, id, entry, &repo).await {
            Ok(b) => out_backends.push(b),
            Err(failure) => {
                error!("entry `{id}` failed: {}", failure.error);
                let prior_entry = prior
                    .as_ref()
                    .and_then(|p| p.backends.iter().find(|b| b.id == *id));
                if let Some(carried) = carryforward::maybe_carry_forward(
                    id,
                    prior_entry,
                    &failure.error,
                    failure.attempted_version.as_deref().unwrap_or(""),
                    failure.attempted_tag.as_deref().unwrap_or(""),
                    &now_iso,
                    carryforward::MAX_STALENESS_DAYS,
                ) {
                    warn!(
                        "entry `{id}` — carrying forward last-known-good (v{})",
                        carried.version
                    );
                    out_backends.push(carried);
                }
            }
        }
    }

    ensure_unique_sources(&out_backends)?;

    let index = index_json::Index {
        schema_version: index_json::SCHEMA_VERSION,
        generated_at: now_iso,
        min_client: index_json::MIN_CLIENT.into(),
        backends: out_backends,
    };
    let text = serde_json::to_string_pretty(&index)?;
    std::fs::write(&args.out, text.as_bytes())
        .with_context(|| format!("writing {}", args.out.display()))?;
    info!(
        "wrote {} ({} backends)",
        args.out.display(),
        index.backends.len()
    );
    Ok(())
}

async fn build_entry(
    client: &dyn ForgeClient,
    http: &reqwest::Client,
    id: &str,
    entry: &registry_toml::Entry,
    repo: &RepoRef,
) -> Result<index_json::IndexBackend, BuildFailure> {
    let resolved = resolve::resolve(client, repo, entry)
        .await
        .map_err(|e| BuildFailure {
            error: format!("{e:#}"),
            attempted_version: None,
            attempted_tag: None,
        })?;
    // From here the version + tag are known; record them on every later failure
    // so the carry-forward path can report what it tried to build.
    let attempted_version = Some(resolved.version.to_string());
    let attempted_tag = Some(resolved.tag.clone());
    let fail = |e: &dyn std::fmt::Display| BuildFailure {
        error: format!("{e:#}"),
        attempted_version: attempted_version.clone(),
        attempted_tag: attempted_tag.clone(),
    };

    // The manifest is the `backend.toml` release asset: parse + validate the
    // exact bytes that get hashed, so reviewed == pinned == installed. A release
    // without the asset is not installable (no synthesize fallback) — fail the
    // entry.
    let (url, _declared_size) =
        assets::resolve_url("backend.toml", &resolved.release.assets).map_err(|e| fail(&e))?;
    let (bytes, sha256) = assets::fetch_manifest_asset(http, &url)
        .await
        .map_err(|e| fail(&e))?;
    let size = bytes.len() as u64;
    let text = String::from_utf8(bytes).map_err(|e| fail(&e))?;
    let m = manifest::Manifest::parse(&text).map_err(|e| fail(&e))?;
    let manifest_pin = Some(index_json::IndexAsset { url, size, sha256 });
    manifest::validate(&m, &resolved.version, &entry.repo).map_err(|e| fail(&e))?;
    let idx_assets = resolve_index_assets(http, &m, &resolved.release.assets)
        .await
        .map_err(|e| fail(&e))?;

    Ok(into_index_backend(
        id,
        m,
        resolved.version.to_string(),
        resolved.tag,
        idx_assets,
        manifest_pin,
    ))
}

/// Resolve and hash the binary artifacts a release declares — the wasm
/// component or each subprocess variant — into the index's asset block.
async fn resolve_index_assets(
    http: &reqwest::Client,
    m: &manifest::Manifest,
    release_assets: &[super_stt_forge::ReleaseAsset],
) -> anyhow::Result<index_json::IndexAssets> {
    let mut idx_assets = index_json::IndexAssets::default();
    if let Some(wasm) = &m.assets.wasm {
        let (url, size) = assets::resolve_url(wasm, release_assets)?;
        let sha = assets::fetch_and_validate(http, &url, assets::AssetExpect::Wasm { file: wasm })
            .await?;
        idx_assets.wasm = Some(index_json::IndexAsset {
            url,
            size,
            sha256: sha,
        });
    }
    for sa in &m.assets.subprocess {
        let (url, size) = assets::resolve_url(&sa.file, release_assets)?;
        let sha = assets::fetch_and_validate(
            http,
            &url,
            assets::AssetExpect::Subprocess {
                file: &sa.file,
                entrypoint: &m.backend.entrypoint,
            },
        )
        .await?;
        idx_assets
            .subprocess
            .push(index_json::IndexSubprocessAsset {
                target: sa.target.clone(),
                accel: sa.accel.to_string(),
                cuda_major: sa.cuda_major,
                cuda_sm: sa.cuda_sm,
                cudnn: sa.cudnn,
                url,
                size,
                sha256: sha,
            });
    }
    Ok(idx_assets)
}

/// Assemble the published `IndexBackend` from a validated manifest, its
/// resolved `version` + `tag`, and the hashed assets. `online` / `supports_gpu`
/// / `supports_cpu` are derived from the manifest's models. Shared by the
/// forge-release path and the offline `local` path.
#[allow(clippy::similar_names)] // supports_gpu / supports_cpu mirror the output fields
pub(crate) fn into_index_backend(
    id: &str,
    m: manifest::Manifest,
    version: String,
    tag: String,
    assets: index_json::IndexAssets,
    manifest: Option<index_json::IndexAsset>,
) -> index_json::IndexBackend {
    let online = m
        .models
        .iter()
        .any(super_stt_registry_types::manifest::ModelEntry::is_online);
    let supports_gpu = m.models.iter().any(|md| {
        md.supported_devices
            .iter()
            .any(|d| matches!(d, Device::Cuda | Device::Metal))
    });
    let supports_cpu = m
        .models
        .iter()
        .any(|md| md.supported_devices.contains(&Device::Cpu));
    index_json::IndexBackend {
        id: id.into(),
        source: m.backend.source,
        version,
        tag,
        name: m.backend.name,
        description: m.backend.description,
        license: m.backend.license.unwrap_or_default(),
        kind: m.backend.kind.to_string(),
        contract: m.backend.contract.to_string(),
        entrypoint: m.backend.entrypoint,
        allowed_hosts: m.network.allowed_hosts,
        online,
        supports_gpu,
        supports_cpu,
        models: m
            .models
            .into_iter()
            .map(|md| index_json::IndexModel {
                name: md.name,
                provider: md.provider.to_string(),
                supported_devices: md
                    .supported_devices
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            })
            .collect(),
        secrets: m
            .secrets
            .into_iter()
            .map(|s| index_json::IndexSecret {
                label: s.label.unwrap_or_else(|| s.name.clone()),
                name: s.name,
                required: s.required,
            })
            .collect(),
        options: m
            .options
            .into_iter()
            .map(|o| index_json::IndexOption {
                label: o.label.unwrap_or_else(|| o.name.clone()),
                name: o.name,
                r#type: o
                    .r#type
                    .map_or_else(|| "string".to_string(), |t| t.to_string()),
                // Untagged serialize yields the plain JSON value (string/number/bool),
                // exactly what the old serde_json::Value field carried.
                default: o
                    .default
                    .map(|d| serde_json::to_value(d).expect("plain value")),
            })
            .collect(),
        assets,
        index_stale: None,
        manifest,
    }
}

/// A backend's `source` is its unique identity. Two distinct entries that
/// collide on `source` would be indistinguishable to every daemon (the
/// daemon's `dedup_sources` guard silently drops all but the first), so a
/// collision must never be published — fail the build instead.
fn ensure_unique_sources(backends: &[index_json::IndexBackend]) -> anyhow::Result<()> {
    let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for b in backends {
        if let Some(prev_id) = seen.insert(b.source.as_str(), b.id.as_str()) {
            anyhow::bail!(
                "duplicate source `{}` shared by entries `{}` and `{}`; each backend must have a distinct source",
                b.source,
                prev_id,
                b.id,
            );
        }
    }
    Ok(())
}

fn chrono_now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(id: &str, source: &str) -> index_json::IndexBackend {
        index_json::IndexBackend {
            id: id.into(),
            source: source.into(),
            version: "1.0.0".into(),
            tag: "v1.0.0".into(),
            name: id.into(),
            description: None,
            license: "Apache-2.0".into(),
            kind: "wasm".into(),
            contract: "v1".into(),
            entrypoint: format!("{id}.wasm"),
            allowed_hosts: Vec::new(),
            online: false,
            supports_gpu: false,
            supports_cpu: true,
            models: Vec::new(),
            secrets: Vec::new(),
            options: Vec::new(),
            assets: index_json::IndexAssets::default(),
            index_stale: None,
            manifest: None,
        }
    }

    #[test]
    fn unique_sources_pass() {
        let backends = vec![
            backend("mistral", "github.com/x/y/mistral"),
            backend("openai", "github.com/x/y/openai"),
        ];
        ensure_unique_sources(&backends).unwrap();
    }

    #[test]
    fn duplicate_sources_are_rejected() {
        let backends = vec![
            backend("mistral", "github.com/x/y"),
            backend("openai", "github.com/x/y"),
        ];
        let err = ensure_unique_sources(&backends).unwrap_err();
        assert!(err.to_string().contains("duplicate source"));
    }
}
