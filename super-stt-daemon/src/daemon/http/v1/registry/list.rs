// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use std::path::PathBuf;
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

/// Resolve the backends directory from app state.
async fn backends_dir(s: &AppState) -> PathBuf {
    let c = s.daemon.config.read().await;
    c.transcription.backends_dir.clone().map_or_else(
        crate::stt_models::backends::default_backends_dir,
        PathBuf::from,
    )
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

/// Read the installed version for a backend from its `backend.toml`, if present.
/// `None` marks a backend that is not installed here.
fn installed_version(backends_dir: &std::path::Path, backend_id: &str) -> Option<String> {
    crate::stt_models::backends::installed_version(&backends_dir.join(backend_id))
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
    let bdir = backends_dir(&s).await;

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
            installed_version(&bdir, &entry.id),
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
}
