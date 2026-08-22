// SPDX-License-Identifier: GPL-3.0-only
//! Daemon-side registry-index policy. The `index.json` schema itself is the
//! canonical `super-stt-registry-types::index` (shared with the indexer
//! producer and the `/registry/backends` leaf types); this module re-exports
//! those types and adds the two daemon-only extensions over [`Index`] — the
//! `min_client` soft-floor check and the unsafe-path backend filter — as free
//! functions, the same pattern `validate_runtime` uses for `Manifest`.

use semver::Version;

pub use super_stt_registry_types::index::{
    Index, IndexAsset, IndexAssets, IndexBackend, IndexModel, IndexOption, IndexSecret, IndexStale,
    IndexSubprocessAsset, id_from_source,
};

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

/// Warn when this index's `min_client` floor is newer than the running daemon.
/// The registry stays usable — `min_client` is a soft floor.
pub fn warn_if_client_too_old(index: &Index) {
    if let MinClientStatus::TooOld { client, min_client } =
        check_min_client(CLIENT_VERSION, &index.min_client)
    {
        log::warn!(
            "registry index requires client >= {min_client}, but this daemon is \
             {client}; newer backends may fail to install or run — please update Super STT"
        );
    }
}

/// Sanitize backends fetched from `index.json` before anything downstream
/// (in particular [`super::install_dir_name`]) can join them onto the
/// backends directory. These values become directory names / are joined onto
/// the backends dir at install time; an absolute or traversing value would
/// escape it. A well-formed index (the indexer rejects them) never contains
/// such values, so a stray one — e.g. from a poisoned
/// `SUPER_STT_REGISTRY_URL` — is sanitized here rather than failing the
/// whole index.
///
/// `backend_id` is optional and its absence has a well-defined meaning (fall
/// back to the registry key), so a rejected value is cleared to `None` rather
/// than dropping the backend over it. `id` and `entrypoint` are required and
/// have no such fallback, so an entry with an unsafe one of those is dropped
/// outright.
///
/// `backend_id` is held to the full `[backend].id` format rule
/// ([`super_stt_registry_types::backend_id::is_valid`]) rather than to
/// `is_safe_component`. Every other route into an install directory name goes
/// through `Manifest::parse`, which enforces exactly that rule, so a looser
/// check here would make `index.json` the one input the daemon accepts below
/// its own contract. `.staging` shows why that matters: it is a legal path
/// component, so a component-level check passes it, but it names the shared
/// staging root every install stages through.
pub fn retain_safe_backends(index: &mut Index) {
    use super_stt_shared::registry::{is_safe_component, is_safe_relative_path};
    for b in &mut index.backends {
        if let Some(id) = b.backend_id.as_deref()
            && !super_stt_registry_types::backend_id::is_valid(id)
        {
            log::warn!(
                "registry: clearing backend `{}` malformed backend_id {id:?}",
                b.id
            );
            b.backend_id = None;
        }
    }
    index.backends.retain(|b| {
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

    /// A minimal-but-safe backend entry, for exercising `retain_safe_backends`
    /// without constructing the full field list every time.
    fn safe_backend(id: &str, backend_id: Option<&str>) -> IndexBackend {
        IndexBackend {
            id: id.into(),
            backend_id: backend_id.map(String::from),
            source: "github.com/x/y".into(),
            version: "1.0.0".into(),
            tag: "v1.0.0".into(),
            name: "X".into(),
            description: None,
            license: String::new(),
            kind: "subprocess".into(),
            contract: "v1".into(),
            entrypoint: "x".into(),
            allowed_hosts: vec![],
            online: false,
            supports_gpu: false,
            supports_cpu: true,
            models: vec![],
            secrets: vec![],
            options: vec![],
            assets: IndexAssets::default(),
            index_stale: None,
            manifest: None,
        }
    }

    /// `backend_id` arrives over the network via `index.json`. An unsafe value
    /// (here, path traversal) must not survive to reach the install pipeline —
    /// but the entry itself, whose required `id`/`entrypoint` are fine, must
    /// not be dropped over it: the field is optional and clearing it just
    /// falls back to the registry key.
    #[test]
    fn retain_safe_backends_clears_an_unsafe_backend_id_but_keeps_the_entry() {
        let mut index = Index {
            schema_version: 1,
            generated_at: "now".into(),
            min_client: "0.1.0".into(),
            backends: vec![safe_backend("voxtral", Some("../../../../home/jorge/.ssh"))],
        };
        retain_safe_backends(&mut index);
        assert_eq!(index.backends.len(), 1, "the entry itself must survive");
        assert_eq!(index.backends[0].id, "voxtral");
        assert!(
            index.backends[0].backend_id.is_none(),
            "an unsafe backend_id must be cleared, not passed through"
        );
    }

    /// `.staging` passes a component-level safety check but names the shared
    /// staging root every install stages through, so an index that published
    /// it would resolve an install directory onto that root. The boundary
    /// holds `backend_id` to the `[backend].id` format rule, which rejects
    /// it — and the entry itself still survives, falling back to its
    /// registry key.
    #[test]
    fn retain_safe_backends_clears_the_shared_staging_root_as_a_backend_id() {
        assert!(
            super_stt_shared::registry::is_safe_component(".staging"),
            "the premise: a component-level check accepts .staging"
        );
        let mut index = Index {
            schema_version: 1,
            generated_at: "now".into(),
            min_client: "0.1.0".into(),
            backends: vec![safe_backend("voxtral", Some(".staging"))],
        };
        retain_safe_backends(&mut index);
        assert_eq!(index.backends.len(), 1, "the entry itself must survive");
        assert!(
            index.backends[0].backend_id.is_none(),
            ".staging must never reach the install pipeline as a directory name"
        );
    }

    /// Every other route into an install directory name goes through
    /// `Manifest::parse`, which enforces the `[backend].id` format. This
    /// boundary must not be more lenient than the daemon's own contract.
    #[test]
    fn retain_safe_backends_clears_a_malformed_backend_id() {
        for malformed in ["voxtral", "app.voxtral", "app.super_stt.voxtral", "APP.X.Y"] {
            let mut index = Index {
                schema_version: 1,
                generated_at: "now".into(),
                min_client: "0.1.0".into(),
                backends: vec![safe_backend("voxtral", Some(malformed))],
            };
            retain_safe_backends(&mut index);
            assert!(
                index.backends[0].backend_id.is_none(),
                "malformed backend_id {malformed:?} must be cleared"
            );
        }
    }

    #[test]
    fn retain_safe_backends_keeps_a_safe_backend_id() {
        let mut index = Index {
            schema_version: 1,
            generated_at: "now".into(),
            min_client: "0.1.0".into(),
            backends: vec![safe_backend("voxtral", Some("app.super-stt.voxtral"))],
        };
        retain_safe_backends(&mut index);
        assert_eq!(
            index.backends[0].backend_id.as_deref(),
            Some("app.super-stt.voxtral")
        );
    }

    /// Pre-existing behavior, now under explicit test: an unsafe `id` or
    /// `entrypoint` drops the whole entry, since `id` is required and has no
    /// well-defined fallback the way `backend_id` does.
    #[test]
    fn retain_safe_backends_still_drops_an_entry_with_an_unsafe_id() {
        let mut index = Index {
            schema_version: 1,
            generated_at: "now".into(),
            min_client: "0.1.0".into(),
            backends: vec![safe_backend("../evil", None)],
        };
        retain_safe_backends(&mut index);
        assert!(index.backends.is_empty());
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
        warn_if_client_too_old(&mk("9999.0.0")); // exercises the warn branch
        warn_if_client_too_old(&mk("0.1.0")); // exercises the quiet branch
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
