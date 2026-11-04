// SPDX-License-Identifier: GPL-3.0-only
//! Daemon-side registry-index policy. The `index.json` schema itself is the
//! canonical `super-tts-registry-types::index` (shared with the indexer
//! producer and the `/registry/backends` leaf types); the policy over it —
//! the `min_client` soft-floor check and the unsafe-path backend filter — is
//! `super_engine_daemon::registry::index`, shared with Super STT.

pub use super_engine_daemon::registry::index::{
    MinClientStatus, check_min_client, retain_safe_backends,
};
pub use super_tts_registry_types::index::{
    Index, IndexAsset, IndexAssets, IndexBackend, IndexModel, IndexOption, IndexSecret, IndexStale,
    IndexSubprocessAsset, id_from_source,
};

/// The running daemon's version, used as the "client" version when checking an
/// index's `min_client` soft floor. This is the workspace version baked in at
/// build time.
pub const CLIENT_VERSION: &str = super::DAEMON.version;

#[cfg(test)]
mod tests {
    //! End-to-end check that the indexer's offline `local` mode produces JSON
    //! the daemon can read — guarding drift between the indexer's `index_json`
    //! output and this module's `Index` input. Generates an index from the
    //! `tests/fixtures` dummy manifest with the real binary, then deserializes
    //! it with the `Index` type the registry client uses. Skips gracefully when
    //! the indexer binary hasn't been built (e.g. `cargo test -p super-tts-daemon`
    //! on its own); `cargo test --workspace` builds it and runs this.
    use super::*;
    use semver::Version;
    use std::path::PathBuf;
    use std::process::Command;

    /// The `super-tts-indexer` binary sibling to this test binary
    /// (`target/<profile>/super-tts-indexer`), if it has been built.
    fn indexer_bin() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        // .../target/<profile>/deps/<testbin> -> .../target/<profile>
        let bin = exe.parent()?.parent()?.join("super-tts-indexer");
        bin.exists().then_some(bin)
    }

    #[test]
    fn reads_generated_test_index_end_to_end() {
        let Some(indexer) = indexer_bin() else {
            eprintln!("skipping: super-tts-indexer binary not built");
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
    fn current_client_meets_the_published_floor() {
        // The value pinned in the indexer
        // (`super-tts-indexer/src/index_json.rs`). The running
        // daemon must satisfy it, or every install would warn against its own
        // registry.
        assert_eq!(
            check_min_client(CLIENT_VERSION, "0.1.0"),
            MinClientStatus::Compatible
        );
    }
}
