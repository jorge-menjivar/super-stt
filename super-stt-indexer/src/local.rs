// SPDX-License-Identifier: GPL-3.0-only
//! Offline index generation: build an `index.json` from locally-staged
//! backends, for testing the daemon's download/install pipeline without GitHub
//! or Pages. Produces the same `index.json` shape as the published `build`
//! path — via the shared [`crate::into_index_backend`] — so the two can never
//! drift. Only the assets differ: each declared artifact is found in the output
//! directory, hashed locally, and given a URL under a local static server
//! instead of a GitHub release asset.
//!
//! `super-stt-indexer local --out <dir> [--base-url <url>] [--allow-missing-assets] <backend.toml>...`

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::Args;
use log::info;

use crate::index_json::{self, Index, IndexAsset, IndexAssets, IndexBackend};
use crate::manifest::{Kind, Manifest};

/// All-zero SHA-256, emitted for assets not staged on disk under
/// `--allow-missing-assets` (listing/read tests that don't exercise install).
const PLACEHOLDER_SHA: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Args, Debug)]
pub struct LocalArgs {
    /// Output directory — also where built assets are staged. `index.json` is
    /// written here.
    #[arg(long)]
    pub out: PathBuf,
    /// Base URL a static server will serve the output directory at.
    #[arg(long, default_value = "http://localhost:8787")]
    pub base_url: String,
    /// Emit a placeholder size/sha for assets not staged on disk — for
    /// listing/read tests that don't download or install.
    #[arg(long)]
    pub allow_missing_assets: bool,
    /// `backend.toml` paths to include.
    #[arg(required = true)]
    pub manifests: Vec<PathBuf>,
}

/// Build the local index and write `<out>/index.json`.
///
/// # Errors
/// Returns an error if the output directory can't be created, a manifest can't
/// be read or parsed, or a required asset is missing (without
/// `--allow-missing-assets`).
pub fn run(args: &LocalArgs) -> anyhow::Result<()> {
    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("creating {}", args.out.display()))?;
    let base = args.base_url.trim_end_matches('/');

    let mut backends = Vec::with_capacity(args.manifests.len());
    for path in &args.manifests {
        let b = build_entry(path, &args.out, base, args.allow_missing_assets)
            .with_context(|| format!("building local entry from {}", path.display()))?;
        backends.push(b);
    }

    let index = Index {
        schema_version: index_json::SCHEMA_VERSION,
        generated_at: crate::chrono_now_iso(),
        min_client: index_json::MIN_CLIENT.into(),
        backends,
    };
    let out_path = args.out.join("index.json");
    let text = serde_json::to_string_pretty(&index)? + "\n";
    std::fs::write(&out_path, text.as_bytes())
        .with_context(|| format!("writing {}", out_path.display()))?;
    info!(
        "wrote {} ({} backends)",
        out_path.display(),
        index.backends.len()
    );
    Ok(())
}

fn build_entry(
    path: &Path,
    out_dir: &Path,
    base: &str,
    allow_missing: bool,
) -> anyhow::Result<IndexBackend> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let m = Manifest::parse(&text)?;
    let assets = resolve_assets(&m, out_dir, base, allow_missing)?;
    let version = m.backend.version.clone();
    let tag = format!("v{version}");
    let id = local_id(&m.backend.source);
    // Stage the manifest as a served asset and pin it, so the offline index
    // exercises the daemon's verified-manifest install path end to end. Named
    // per id to avoid collisions when several backends share one output dir.
    let manifest = stage_manifest(&id, &text, out_dir, base)?;
    Ok(crate::into_index_backend(
        &id, m, version, tag, assets, manifest,
    ))
}

/// Write `<out>/<id>.backend.toml` and pin it as the manifest asset.
fn stage_manifest(
    id: &str,
    text: &str,
    out_dir: &Path,
    base: &str,
) -> anyhow::Result<Option<IndexAsset>> {
    let file = format!("{id}.backend.toml");
    let staged = out_dir.join(&file);
    std::fs::write(&staged, text).with_context(|| format!("writing {}", staged.display()))?;
    let sha256 = hex::encode(ring::digest::digest(&ring::digest::SHA256, text.as_bytes()));
    Ok(Some(IndexAsset {
        url: format!("{base}/{file}"),
        size: text.len() as u64,
        sha256,
    }))
}

/// The registry keys entries by the last path segment of `source`; mirror that
/// so a locally-built index uses the same id the published one would.
fn local_id(source: &str) -> String {
    source
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(source)
        .to_string()
}

fn resolve_assets(
    m: &Manifest,
    out_dir: &Path,
    base: &str,
    allow_missing: bool,
) -> anyhow::Result<IndexAssets> {
    let mut assets = IndexAssets::default();
    match m.backend.kind {
        Kind::Wasm => {
            let file = m
                .assets
                .wasm
                .as_deref()
                .context("wasm backend declares no `[assets].wasm`")?;
            let Some((size, sha256)) = hash_staged(out_dir, file, allow_missing)? else {
                bail!(
                    "staged asset `{file}` not found in {} — build and stage it, or pass --allow-missing-assets",
                    out_dir.display()
                );
            };
            assets.wasm = Some(IndexAsset {
                url: format!("{base}/{file}"),
                size,
                sha256,
            });
        }
        Kind::Subprocess => {
            for a in &m.assets.subprocess {
                // A test box rarely has every CUDA build staged; skip a variant
                // any of whose parts isn't present (unless placeholdering).
                let files = a.release_files();
                let mut pins: Vec<IndexAsset> = Vec::with_capacity(files.len());
                let mut missing = false;
                for f in &files {
                    let Some((size, sha256)) = hash_staged(out_dir, f, allow_missing)? else {
                        missing = true;
                        break;
                    };
                    pins.push(IndexAsset {
                        url: format!("{base}/{f}"),
                        size,
                        sha256,
                    });
                }
                if missing {
                    continue;
                }
                assets
                    .subprocess
                    .push(crate::subprocess_index_entry(a, pins));
            }
            if assets.subprocess.is_empty() {
                bail!(
                    "no subprocess assets for `{}` staged in {}",
                    m.backend.source,
                    out_dir.display()
                );
            }
        }
    }
    Ok(assets)
}

/// `Some((size, sha256))` for a staged file; a placeholder when the file is
/// missing and `allow_missing`; `None` when missing and not placeholdering, so
/// the caller decides whether that's a hard error (wasm) or a skip (one of many
/// subprocess variants).
fn hash_staged(
    out_dir: &Path,
    file: &str,
    allow_missing: bool,
) -> anyhow::Result<Option<(u64, String)>> {
    let staged = out_dir.join(file);
    if staged.exists() {
        Ok(Some(sha256_and_size(&staged)?))
    } else if allow_missing {
        Ok(Some((0, PLACEHOLDER_SHA.to_string())))
    } else {
        Ok(None)
    }
}

fn sha256_and_size(path: &Path) -> anyhow::Result<(u64, String)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 64 * 1024];
    let mut size: u64 = 0;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        ctx.update(&buf[..n]);
    }
    Ok((size, hex::encode(ctx.finish().as_ref())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_id_takes_last_source_segment() {
        assert_eq!(local_id("github.com/jorge-menjivar/dummy"), "dummy");
        assert_eq!(local_id("github.com/x/y/openai"), "openai");
        assert_eq!(local_id("github.com/x/y/"), "y");
    }

    #[test]
    fn builds_a_wasm_index_with_placeholder_when_asset_missing() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("backend.toml");
        std::fs::write(
            &manifest,
            r#"
            [backend]
            source = "github.com/x/dummy"
            name = "Dummy"
            version = "1.2.3"
            kind = "wasm"
            entrypoint = "dummy.wasm"
            contract = "v1"
            description = "Test backend."

            [assets]
            wasm = "dummy.wasm"

            [[models]]
            name = "m"
            provider = "openai"
            primary_language = "en"
            supported_languages = ["en"]
            supported_devices = ["none"]
            "#,
        )
        .unwrap();
        let args = LocalArgs {
            out: dir.path().to_path_buf(),
            base_url: "http://localhost:8787".into(),
            allow_missing_assets: true,
            manifests: vec![manifest],
        };
        run(&args).unwrap();

        let json = std::fs::read_to_string(dir.path().join("index.json")).unwrap();
        let index: Index = serde_json::from_str(&json).unwrap();
        assert_eq!(index.backends.len(), 1);
        let b = &index.backends[0];
        assert_eq!(b.id, "dummy");
        assert_eq!(b.tag, "v1.2.3");
        assert!(b.online, "an only-`none` model marks the backend online");
        let wasm = b.assets.wasm.as_ref().expect("wasm asset");
        assert_eq!(wasm.url, "http://localhost:8787/dummy.wasm");
        assert_eq!(wasm.sha256.len(), 64);
        let manifest = b.manifest.as_ref().expect("manifest is pinned");
        assert_eq!(manifest.url, "http://localhost:8787/dummy.backend.toml");
        assert_eq!(manifest.sha256.len(), 64);
        assert!(dir.path().join("dummy.backend.toml").exists());
    }

    #[test]
    fn hashes_a_real_staged_asset() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.wasm"), b"hello").unwrap();
        let (size, sha) = sha256_and_size(&dir.path().join("a.wasm")).unwrap();
        assert_eq!(size, 5);
        // sha256("hello")
        assert_eq!(
            sha,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
