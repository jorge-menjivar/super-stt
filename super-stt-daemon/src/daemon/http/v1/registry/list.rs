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
fn installed_version(backends_dir: &std::path::Path, backend_id: &str) -> Option<String> {
    let candidate = backends_dir.join(backend_id).join("backend.toml");
    if candidate.exists() {
        crate::stt_models::backends::manifest::Manifest::load(&backends_dir.join(backend_id))
            .ok()
            .map(|m| m.backend.version)
    } else {
        None
    }
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
