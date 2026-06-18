// SPDX-License-Identifier: GPL-3.0-only
//! Deserialization shape for `index.json` as published by the Phase 1 indexer.
//! Kept in sync with `super-stt-indexer/src/index_json.rs`. The
//! daemon side does not need every field — those it ignores are skipped via
//! `serde(default)`.

use semver::Version;
use serde::Deserialize;

/// The running daemon's version, used as the "client" version when checking an
/// index's `min_client` soft floor. This is the workspace version baked in at
/// build time.
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Outcome of checking the running client against an index's `min_client`
/// soft floor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MinClientStatus {
    /// The client meets the floor, or the comparison cannot be made (an absent
    /// or unparseable `min_client`, or an unparseable client version). A
    /// malformed floor must never take the registry offline.
    Compatible,
    /// The client is older than the index's declared minimum. The registry
    /// stays usable; the user should be warned to update.
    TooOld { client: String, min_client: String },
}

/// Compare a client version against an index's `min_client` soft floor using
/// standard semver precedence. A missing or unparseable `min_client`, or an
/// unparseable `client_version`, yields [`MinClientStatus::Compatible`] — a bad
/// version string must not disable the registry.
#[must_use]
pub fn check_min_client(client_version: &str, min_client: &str) -> MinClientStatus {
    let (Ok(client), Ok(min)) = (Version::parse(client_version), Version::parse(min_client)) else {
        return MinClientStatus::Compatible;
    };
    if client < min {
        MinClientStatus::TooOld {
            client: client_version.to_owned(),
            min_client: min_client.to_owned(),
        }
    } else {
        MinClientStatus::Compatible
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Index {
    pub schema_version: u32,
    pub generated_at: String,
    pub min_client: String,
    pub backends: Vec<IndexBackend>,
}

impl Index {
    /// Warn when this index's `min_client` floor is newer than the running
    /// daemon. The registry stays usable — `min_client` is a soft floor.
    pub fn warn_if_client_too_old(&self) {
        if let MinClientStatus::TooOld { client, min_client } =
            check_min_client(CLIENT_VERSION, &self.min_client)
        {
            log::warn!(
                "registry index requires client >= {min_client}, but this daemon is \
                 {client}; newer backends may fail to install or run — please update Super STT"
            );
        }
    }

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
    /// Pinned `backend.toml` release asset. When present, the installer fetches
    /// these exact bytes, verifies them against `sha256`, and installs them
    /// verbatim instead of synthesizing a manifest from the fields above.
    #[serde(default)]
    pub manifest: Option<IndexAsset>,
}

/// The browse-only model subset the catalog and host-compatibility filter need.
/// The authoritative manifest ships as the pinned `manifest` asset and is
/// installed verbatim — it is not re-encoded here.
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
    /// Single-file archive pin: present for a single-file variant, absent for
    /// multi-part.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub sha256: Option<String>,
    /// Multi-part archive: ordered part pins whose byte-for-byte concatenation
    /// is the `.tar.gz`. Present for a multi-part variant, absent for
    /// single-file. Each part is hash-verified independently on download.
    #[serde(default)]
    pub parts: Vec<IndexAsset>,
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
    //! End-to-end check that the indexer's offline `local` mode produces JSON
    //! the daemon can read — guarding drift between the indexer's `index_json`
    //! output and this module's `Index` input. Generates an index from the
    //! `tests/fixtures` dummy manifest with the real binary, then deserializes
    //! it with the `Index` type the registry client uses. Skips gracefully when
    //! the indexer binary hasn't been built (e.g. `cargo test -p super-stt-daemon`
    //! on its own); `cargo test --workspace` builds it and runs this.
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// The `super-stt-indexer` binary sibling to this test binary
    /// (`target/<profile>/super-stt-indexer`), if it has been built.
    fn indexer_bin() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        // .../target/<profile>/deps/<testbin> -> .../target/<profile>
        let bin = exe.parent()?.parent()?.join("super-stt-indexer");
        bin.exists().then_some(bin)
    }

    #[test]
    fn reads_generated_test_index_end_to_end() {
        let Some(indexer) = indexer_bin() else {
            eprintln!("skipping: super-stt-indexer binary not built");
            return;
        };
        let dummy =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dummy-backend.toml");
        assert!(
            dummy.exists(),
            "dummy manifest missing at {}",
            dummy.display()
        );

        let out = tempfile::tempdir().unwrap();
        let status = Command::new(&indexer)
            .arg("local")
            .arg("--out")
            .arg(out.path())
            .arg("--base-url")
            .arg("http://localhost:8787")
            .arg("--allow-missing-assets")
            .arg(&dummy)
            .status()
            .expect("run indexer");
        assert!(status.success(), "indexer exited with failure");

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

    #[test]
    fn the_daemons_own_version_is_valid_semver() {
        // The client side of the comparison must always parse, so a real
        // floor is never silently ignored as "unparseable client".
        assert!(
            Version::parse(CLIENT_VERSION).is_ok(),
            "CLIENT_VERSION {CLIENT_VERSION:?} must be valid semver"
        );
    }

    #[test]
    fn client_at_or_above_floor_is_compatible() {
        for (client, floor) in [
            ("0.1.0", "0.1.0"),        // exact floor is allowed (>=)
            ("0.2.0", "0.1.0"),        // newer minor
            ("1.0.0", "0.1.0"),        // newer major
            ("0.1.4-beta.2", "0.1.0"), // current beta: 0.1.4 core > 0.1.0
        ] {
            assert_eq!(
                check_min_client(client, floor),
                MinClientStatus::Compatible,
                "{client} should meet floor {floor}"
            );
        }
    }

    #[test]
    fn client_below_floor_is_too_old() {
        assert_eq!(
            check_min_client("0.0.9", "0.1.0"),
            MinClientStatus::TooOld {
                client: "0.0.9".into(),
                min_client: "0.1.0".into(),
            }
        );
        // Standard semver: a prerelease of the floor ranks below the release.
        assert!(matches!(
            check_min_client("0.1.0-rc.1", "0.1.0"),
            MinClientStatus::TooOld { .. }
        ));
    }

    #[test]
    fn malformed_versions_never_block_the_registry() {
        // A bad floor or client version must degrade to Compatible, not take
        // the whole registry offline.
        for (client, floor) in [
            ("0.1.4-beta.2", "not-a-version"),
            ("0.1.4-beta.2", ""),
            ("garbage", "0.1.0"),
        ] {
            assert_eq!(
                check_min_client(client, floor),
                MinClientStatus::Compatible,
                "{client:?} vs {floor:?} must not block"
            );
        }
    }

    #[test]
    fn current_client_meets_the_published_floor() {
        // The value pinned in the indexer
        // (`super-stt-indexer/src/index_json.rs`). The running
        // daemon must satisfy it, or every install would warn against its own
        // registry.
        assert_eq!(
            check_min_client(CLIENT_VERSION, "0.1.0"),
            MinClientStatus::Compatible
        );
    }

    #[test]
    fn index_warns_only_when_too_old() {
        // Drives the real wiring (`Index::warn_if_client_too_old` uses
        // CLIENT_VERSION). A wildly-high floor is too old; the published floor
        // is fine. The method logs as a side effect; we assert the underlying
        // status it acts on.
        let mk = |min_client: &str| Index {
            schema_version: 1,
            generated_at: "now".into(),
            min_client: min_client.into(),
            backends: vec![],
        };
        mk("9999.0.0").warn_if_client_too_old(); // exercises the warn branch
        mk("0.1.0").warn_if_client_too_old(); // exercises the quiet branch
        assert!(matches!(
            check_min_client(CLIENT_VERSION, "9999.0.0"),
            MinClientStatus::TooOld { .. }
        ));
        assert_eq!(
            check_min_client(CLIENT_VERSION, "0.1.0"),
            MinClientStatus::Compatible
        );
    }
}
