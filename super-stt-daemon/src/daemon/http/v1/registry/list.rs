// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use super_stt_shared::registry::{
    Compatibility, RegistryBackend, RegistryListResponse, RegistryModel, RegistryOption,
    RegistrySecret,
};

/// Query parameters for `GET /registry/backends`.
#[derive(Deserialize)]
pub(crate) struct RegistryBackendsQuery {
    #[serde(default)]
    pub(crate) include_incompatible: bool,
    pub(crate) kind: Option<String>,
    pub(crate) online: Option<bool>,
    pub(crate) q: Option<String>,
}

/// Return `true` if `entry` should be included given the query filters.
fn entry_passes_filters(
    entry: &crate::registry::index_schema::IndexBackend,
    q: &RegistryBackendsQuery,
) -> bool {
    if let Some(ref kind_filter) = q.kind
        && &entry.kind != kind_filter
    {
        return false;
    }
    if let Some(online_filter) = q.online
        && entry.online != online_filter
    {
        return false;
    }
    if let Some(ref search) = q.q {
        let s_lower = search.to_lowercase();
        let in_name = entry.name.to_lowercase().contains(&s_lower);
        let in_desc = entry
            .description
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains(&s_lower);
        if !in_name && !in_desc {
            return false;
        }
    }
    true
}

/// The installed version of the backend serving `source`, or `None` when no
/// installed backend claims it.
///
/// The catalog is keyed by `source` because that is the only identifier both
/// sides always carry: the index always has one, and every `backend.toml` must
/// declare one. A directory name does not qualify — it is derived differently
/// depending on which install path produced it, so keying on it silently
/// failed to match anything installed from a custom repo or a local path.
///
/// Re-reads the manifest off disk rather than trusting the cached
/// `DiscoveredBackend::version`, so a version bumped since the last scan is
/// reported; the cached value stands in when that read fails.
///
/// `pub(super)` so `update.rs` shares this instead of re-implementing the same
/// match-by-`source` + fresh-read-with-fallback rule.
pub(super) fn installed_version_for_source(
    backends: &[crate::stt_models::backends::DiscoveredBackend],
    source: &str,
) -> Option<String> {
    let b = backends.iter().find(|b| b.source == source)?;
    Some(
        crate::stt_models::backends::installed_version(&b.dir).unwrap_or_else(|| b.version.clone()),
    )
}

/// Whether the index offers something newer than what is installed.
///
/// The daemon answers this rather than each client re-deriving it: it is the
/// side that reads the installed manifest and owns the index. A backend that is
/// not installed here has no update to offer — it has an *install* — so `None`
/// is `false` rather than "everything is an update".
///
/// The comparison itself is the shared semver one, which already refuses a
/// downgrade and refuses to guess at a version it cannot parse.
fn update_available(installed: Option<&str>, index_version: &str) -> bool {
    installed.is_some_and(|i| super_stt_registry_types::version::update_available(i, index_version))
}

/// Map a registry entry + compatibility result to the wire `RegistryBackend` shape.
fn map_entry(
    entry: &crate::registry::index_schema::IndexBackend,
    compat_field: Compatibility,
    installed_version: Option<String>,
) -> RegistryBackend {
    RegistryBackend {
        id: entry.id.clone(),
        source: entry.source.clone(),
        version: entry.version.clone(),
        name: entry.name.clone(),
        description: entry.description.clone(),
        license: entry.license.clone(),
        kind: entry.kind.clone(),
        contract: entry.contract.clone(),
        allowed_hosts: entry.allowed_hosts.clone(),
        online: entry.online,
        supports_gpu: entry.supports_gpu,
        supports_cpu: entry.supports_cpu,
        models: entry
            .models
            .iter()
            .map(|m| RegistryModel {
                name: m.name.clone(),
                // Compatibility shim; see `IndexModel::provider`. Clients
                // through v0.2.0 require the key to parse this response.
                provider: String::new(),
                supported_devices: m.supported_devices.clone(),
            })
            .collect(),
        secrets: entry
            .secrets
            .iter()
            .map(|s| RegistrySecret {
                name: s.name.clone(),
                label: s.label.clone(),
                required: s.required,
            })
            .collect(),
        options: entry
            .options
            .iter()
            .map(|o| RegistryOption {
                name: o.name.clone(),
                label: o.label.clone(),
                r#type: o.r#type.clone(),
                default: o.default.clone(),
            })
            .collect(),
        compatibility: compat_field,
        update_available: update_available(installed_version.as_deref(), &entry.version),
        installed_version,
        index_stale: entry
            .index_stale
            .as_ref()
            .map(|is| super_stt_shared::registry::IndexStale {
                latest_attempted: is.latest_attempted.clone(),
                tag: is.tag.clone(),
                error: is.error.clone(),
                since: is.since.clone(),
            }),
    }
}

/// `GET /registry/backends` — list installable backends from the registry.
pub(crate) async fn list_registry_backends(
    State(s): State<AppState>,
    Query(q): Query<RegistryBackendsQuery>,
) -> impl IntoResponse {
    use crate::registry::{compat, host_detect};

    let Ok(index) = s.registry_client.get().await else {
        return super::registry_error(StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable");
    };

    let host = host_detect::detect();
    let backends = s.daemon.backends.read().await;

    let mut result = Vec::new();
    for entry in &index.backends {
        if !entry_passes_filters(entry, &q) {
            continue;
        }

        let sel = compat::select(&host, entry);
        let compatible = !matches!(sel, compat::Selection::Incompatible { .. });

        if !compatible && !q.include_incompatible {
            continue;
        }

        let selected_asset = compat::to_selected_asset(entry, &sel);
        let reason = if let compat::Selection::Incompatible { ref reason } = sel {
            Some(reason.clone())
        } else {
            None
        };

        let compat_field = Compatibility {
            compatible,
            selected_asset,
            reason,
        };

        result.push(map_entry(
            entry,
            compat_field,
            installed_version_for_source(&backends, &entry.source),
        ));
    }

    let resp = RegistryListResponse {
        schema_version: index.schema_version,
        generated_at: index.generated_at.clone(),
        backends: result,
    };

    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string(&resp).unwrap_or_default(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::update_available;

    /// The daemon's own part of the decision. The semver comparison is tested
    /// in `super-stt-registry-types`; what is pinned here is what this adds to
    /// it — that "not installed" is not an update, and that a downgrade or an
    /// unreadable version never becomes one on the way to the wire.
    #[test]
    fn only_an_installed_older_version_has_an_update() {
        assert!(update_available(Some("0.1.0"), "0.1.1"));
        assert!(update_available(Some("v1.0.0"), "1.0.1"));

        // Not installed: the client is offered an install, not an update.
        assert!(!update_available(None, "0.1.1"));
        // Already current, and a stale index that would prompt a downgrade.
        assert!(!update_available(Some("0.1.1"), "0.1.1"));
        assert!(!update_available(Some("0.2.0"), "0.1.1"));
        // Neither side is guessed at when it cannot be parsed.
        assert!(!update_available(Some("1.0.0"), "nightly"));
        assert!(!update_available(Some(""), "1.0.0"));
    }

    use crate::stt_models::backends::DiscoveredBackend;
    use std::path::PathBuf;

    fn discovered(dir: &str, source: &str, version: &str) -> DiscoveredBackend {
        DiscoveredBackend {
            dir: PathBuf::from("/backends").join(dir),
            source: source.to_string(),
            name: "Voxtral".to_string(),
            version: version.to_string(),
            kind: "subprocess".to_string(),
            entrypoint: "super-stt-backend-voxtral".to_string(),
            allowed_hosts: Vec::new(),
            secrets: Vec::new(),
            options: Vec::new(),
            models: Vec::new(),
        }
    }

    /// The regression this task exists for: the install directory is named
    /// after the repo, the index id is `voxtral`, and the two never matched.
    /// Matching on `source` is what makes a custom-path install updatable.
    #[test]
    fn a_directory_not_named_after_the_index_id_still_reports_its_version() {
        let catalog = vec![discovered(
            "super-stt-voxtral",
            "github.com/jorge-menjivar/super-stt-voxtral",
            "0.1.0",
        )];
        assert_eq!(
            super::installed_version_for_source(
                &catalog,
                "github.com/jorge-menjivar/super-stt-voxtral"
            ),
            Some("0.1.0".to_string())
        );
        assert!(update_available(Some("0.1.0"), "0.1.1"));
    }

    #[test]
    fn a_source_absent_from_the_catalog_has_no_installed_version() {
        let catalog = vec![discovered(
            "whisper",
            "github.com/x/super-stt-whisper",
            "0.1.0",
        )];
        assert_eq!(
            super::installed_version_for_source(&catalog, "github.com/x/super-stt-voxtral"),
            None
        );
    }
}
