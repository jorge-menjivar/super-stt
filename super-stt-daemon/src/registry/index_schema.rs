// SPDX-License-Identifier: GPL-3.0-only
//! Deserialization shape for `index.json` as published by the Phase 1 indexer.
//! Kept in sync with `registry/scripts/build_index/src/index_json.rs`. The
//! daemon side does not need every field — those it ignores are skipped via
//! `serde(default)`.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Index {
    pub schema_version: u32,
    pub generated_at: String,
    pub min_client: String,
    pub backends: Vec<IndexBackend>,
}

impl Index {
    /// Drop backends whose `id` or `entrypoint` is not a safe path component.
    /// These values become directory names / are joined onto the backends dir
    /// at install time; an absolute or traversing value would escape it. A
    /// well-formed index (the indexer rejects them) never contains such a
    /// backend, so a stray one — e.g. from a poisoned `SUPER_STT_REGISTRY_URL`
    /// — is dropped with a warning rather than failing the whole index.
    pub fn retain_safe_backends(&mut self) {
        use super_stt_shared::registry::{is_safe_component, is_safe_relative_path};
        self.backends.retain(|b| {
            let ok = is_safe_component(&b.id) && is_safe_relative_path(&b.entrypoint);
            if !ok {
                log::warn!(
                    "registry: dropping backend `{}` with unsafe id/entrypoint (entrypoint={:?})",
                    b.id,
                    b.entrypoint
                );
            }
            ok
        });
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexBackend {
    pub id: String,
    pub source: String,
    pub version: String,
    pub tag: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub license: String,
    pub kind: String,
    pub contract: String,
    pub entrypoint: String,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    pub online: bool,
    pub supports_gpu: bool,
    pub supports_cpu: bool,
    pub models: Vec<IndexModel>,
    pub secrets: Vec<IndexSecret>,
    pub options: Vec<IndexOption>,
    pub assets: IndexAssets,
    #[serde(default)]
    pub index_stale: Option<IndexStale>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexModel {
    pub name: String,
    pub provider: String,
    pub supported_devices: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexSecret {
    pub name: String,
    pub label: String,
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexOption {
    pub name: String,
    pub label: String,
    pub r#type: String,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct IndexAssets {
    #[serde(default)]
    pub wasm: Option<IndexAsset>,
    #[serde(default)]
    pub subprocess: Vec<IndexSubprocessAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexAsset {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexSubprocessAsset {
    pub target: String,
    pub accel: String,
    #[serde(default)]
    pub cuda_major: Option<u32>,
    #[serde(default)]
    pub cuda_sm: Option<u32>,
    #[serde(default)]
    pub cudnn: bool,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexStale {
    pub latest_attempted: String,
    pub tag: String,
    pub error: String,
    pub since: String,
}

#[cfg(test)]
mod tests {
    //! End-to-end check that the offline test-index generator
    //! (`registry/scripts/gen_test_index.py`) produces JSON the daemon can
    //! read. Generates an index from the committed dummy manifest, then
    //! deserializes it with the real `Index` type the registry client uses.
    //! Skips gracefully if `python3` isn't on PATH.
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn repo_root() -> PathBuf {
        // CARGO_MANIFEST_DIR is `<repo>/super-stt-daemon`.
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    fn python_available() -> bool {
        Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn reads_generated_test_index_end_to_end() {
        if !python_available() {
            eprintln!("skipping: python3 not available");
            return;
        }
        let root = repo_root();
        let script = root.join("registry/scripts/gen_test_index.py");
        let dummy = root.join("registry/scripts/fixtures/dummy-backend.toml");
        assert!(
            script.exists(),
            "generator script missing at {}",
            script.display()
        );
        assert!(
            dummy.exists(),
            "dummy manifest missing at {}",
            dummy.display()
        );

        let out = tempfile::tempdir().unwrap();
        let status = Command::new("python3")
            .arg(&script)
            .arg("--out")
            .arg(out.path())
            .arg("--base-url")
            .arg("http://localhost:8787")
            .arg("--allow-missing-assets")
            .arg(&dummy)
            .status()
            .expect("run generator");
        assert!(status.success(), "generator exited with failure");

        let json = std::fs::read_to_string(out.path().join("index.json")).unwrap();
        // The real reader: the exact type the registry client uses to parse
        // what it fetches over HTTP.
        let index: Index = serde_json::from_str(&json).expect("daemon must parse generated index");

        assert_eq!(index.schema_version, 1);
        assert_eq!(index.backends.len(), 1);
        let b = &index.backends[0];
        assert_eq!(b.id, "dummy");
        assert_eq!(b.source, "github.com/jorge-menjivar/dummy");
        assert_eq!(b.version, "1.2.3");
        assert_eq!(b.kind, "wasm");
        assert_eq!(b.entrypoint, "dummy.wasm");
        assert!(b.online);
        assert!(b.supports_cpu);
        assert!(b.supports_gpu);
        assert_eq!(b.allowed_hosts, vec!["api.example.com".to_string()]);
        assert_eq!(b.secrets.len(), 1);
        assert!(b.secrets[0].required);
        assert_eq!(b.models.len(), 2);
        let wasm = b.assets.wasm.as_ref().expect("wasm asset present");
        assert_eq!(wasm.url, "http://localhost:8787/dummy.wasm");
        assert_eq!(wasm.sha256.len(), 64);
    }
}
