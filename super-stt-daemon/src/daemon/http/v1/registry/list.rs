// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope, RegistryError};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use super_stt_shared::registry::{
    Compatibility, RegistryBackend, RegistryListResponse, RegistryModel, RegistryOption,
    RegistrySecret,
};

/// Query parameters for `GET /registry/backends`.
#[derive(Deserialize, utoipa::IntoParams)]
pub(crate) struct RegistryBackendsQuery {
    /// Include entries this machine cannot run. Off by default, so the catalog
    /// shows what is actually installable here.
    #[serde(default)]
    pub(crate) include_incompatible: bool,
    /// Filter by backend kind.
    pub(crate) kind: Option<String>,
    /// Filter by whether the backend calls out to a network service.
    pub(crate) online: Option<bool>,
    /// Case-insensitive substring match over name and description.
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

/// Whether the index offers something newer than what is installed *and this
/// daemon could install it*.
///
/// The daemon answers this rather than each client re-deriving it: it is the
/// side that reads the installed manifest and owns the index. A backend that is
/// not installed here has no update to offer — it has an *install* — so `None`
/// is `false` rather than "everything is an update".
///
/// `compatible` is part of the answer, not a separate concern. A newer release
/// this host cannot run is not an update the user can take: offering it puts an
/// Update button on a card whose only outcome is `422 incompatible`. The
/// commonest way for that to happen now is a release that moved to a contract
/// generation this daemon predates — precisely the case where the user needs to
/// update Super STT, not the backend.
///
/// The comparison itself is the shared semver one, which already refuses a
/// downgrade and refuses to guess at a version it cannot parse.
fn update_available(installed: Option<&str>, index_version: &str, compatible: bool) -> bool {
    compatible
        && installed
            .is_some_and(|i| super_stt_registry_types::version::update_available(i, index_version))
}

/// Map a registry entry + compatibility result to the wire `RegistryBackend` shape.
fn map_entry(
    entry: &crate::registry::index_schema::IndexBackend,
    compat_field: Compatibility,
    installed_version: Option<String>,
) -> RegistryBackend {
    RegistryBackend {
        id: entry.id.clone(),
        backend_id: entry.backend_id.clone(),
        source: entry.source.clone(),
        version: entry.version.clone(),
        name: entry.name.clone(),
        description: entry.description.clone(),
        license: entry.license.clone(),
        kind: entry.kind.clone(),
        contract: entry.contract.clone(),
        min_client: entry.min_client.clone(),
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
                role: m.role.clone(),
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
        update_available: update_available(
            installed_version.as_deref(),
            &entry.version,
            compat_field.compatible,
        ),
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
#[utoipa::path(
    get,
    path = "/registry/backends",
    tag = "registry",
    summary = "Browse the published backend catalog",
    description = "\
Every backend published to the registry, with the models each serves and whether \
this machine can run it. `compatible` is decided against the host's actual \
accelerators and this Super STT's version, so the list reflects what is installable \
here rather than what exists in general.

`needs_client_update` separates the two ways a backend can be blocked: a host that \
lacks the right GPU will never run it, but a Super STT one version behind is \
something the user can fix in a minute. Surface those differently.

This is what is *available*; `GET /backends` is what is installed.",
    params(RegistryBackendsQuery),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The catalog.", body = RegistryListResponse),
        (status = 503, description = "The catalog could not be fetched and nothing is cached (`registry_unavailable`). Retry, or force a fetch with `POST /registry/backends/refresh`.", body = RegistryError),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
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
        let compatible = sel.reason().is_none();
        let needs_client_update = sel.needs_client_update();

        // A client-update block is always listed: `include_incompatible` is
        // about hardware this host will never satisfy, and swallowing "your
        // Super STT is too old" behind the same toggle hides the one notice
        // that would tell the user what to do.
        if !compatible && !needs_client_update && !q.include_incompatible {
            continue;
        }

        let compat_field = Compatibility {
            compatible,
            selected_asset: compat::to_selected_asset(entry, &sel),
            reason: sel.reason().map(ToOwned::to_owned),
            needs_client_update,
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
        assert!(update_available(Some("0.1.0"), "0.1.1", true));
        assert!(update_available(Some("v1.0.0"), "1.0.1", true));

        // Not installed: the client is offered an install, not an update.
        assert!(!update_available(None, "0.1.1", true));
        // Already current, and a stale index that would prompt a downgrade.
        assert!(!update_available(Some("0.1.1"), "0.1.1", true));
        assert!(!update_available(Some("0.2.0"), "0.1.1", true));
        // Neither side is guessed at when it cannot be parsed.
        assert!(!update_available(Some("1.0.0"), "nightly", true));
        assert!(!update_available(Some(""), "1.0.0", true));
    }

    /// A newer release this daemon cannot install is not an update on offer.
    /// The Update button it would otherwise draw leads only to
    /// `422 incompatible` — and when the cause is a contract generation this
    /// build predates, updating the *backend* was never the fix.
    #[test]
    fn an_incompatible_release_is_not_offered_as_an_update() {
        assert!(update_available(Some("0.1.0"), "0.2.0", true));
        assert!(!update_available(Some("0.1.0"), "0.2.0", false));
    }

    use crate::stt_models::backends::DiscoveredBackend;
    use std::path::PathBuf;

    fn discovered(dir: &str, source: &str, version: &str) -> DiscoveredBackend {
        DiscoveredBackend {
            description: String::new(),
            dir: PathBuf::from("/backends").join(dir),
            source: source.to_string(),
            id: None,
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
        assert!(update_available(Some("0.1.0"), "0.1.1", true));
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
