// SPDX-License-Identifier: GPL-3.0-only
use super::pipeline::{InflightMarker, spawn_install_pipeline};
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

/// Request body for `POST /registry/backends/update`.
#[derive(Deserialize)]
pub(crate) struct UpdateBody {
    source: String,
}

// ---------------------------------------------------------------------------
// Phase helpers
// ---------------------------------------------------------------------------

// Convenience alias: helpers return boxed responses so the `Err` variant
// stays pointer-sized and does not trip `clippy::result_large_err`.
type ErrResp = Box<axum::response::Response>;

/// Phase 1 — Parse the request body.
fn parse_update_body(raw: Option<axum::Json<UpdateBody>>) -> Result<UpdateBody, ErrResp> {
    let Some(axum::Json(body)) = raw else {
        return Err(Box::new(super::registry_error(
            StatusCode::BAD_REQUEST,
            "missing_body",
        )));
    };
    Ok(body)
}

/// Phase 2 — Acquire the inflight marker (conflict check + insert).
///
/// Check+insert is atomic under one write lock. Returns `Err` with a `409` when
/// an update is already in progress; otherwise an [`InflightMarker`] whose
/// `Drop` removes the marker, so the later synchronous phases (and the no-op
/// early return) bail with a bare `return` and cleanup is automatic. The happy
/// path defuses it before spawning.
fn acquire_update_inflight(s: &AppState, source: &str) -> Result<InflightMarker, ErrResp> {
    InflightMarker::acquire(Arc::clone(&s.install_inflight), source.to_owned()).ok_or_else(|| {
        Box::new(super::registry_error(
            StatusCode::CONFLICT,
            "update_in_progress",
        ))
    })
}

/// Outcome of the version-lookup phase.
struct VersionLookup {
    entry: crate::registry::index_schema::IndexBackend,
    from_version: String,
}

/// Phase 3 — Fetch the registry entry and determine the installed version.
///
/// Returns an HTTP error on any failure (registry unavailable, source not in
/// index, not installed on disk); the caller's [`InflightMarker`] cleans up the
/// inflight set.
async fn lookup_versions(s: &AppState, source: &str) -> Result<VersionLookup, ErrResp> {
    let backends_dir = {
        let c = s.daemon.config.read().await;
        c.transcription.backends_dir.clone().map_or_else(
            crate::stt_models::backends::default_backends_dir,
            PathBuf::from,
        )
    };

    let Ok(index) = s.registry_client.get().await else {
        return Err(Box::new(super::registry_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "registry_unavailable",
        )));
    };

    let Some(entry) = index.backends.iter().find(|b| b.source == source).cloned() else {
        return Err(Box::new(super::registry_error(
            StatusCode::NOT_FOUND,
            "not_found",
        )));
    };

    // Determine installed version from disk.
    let installed_version: Option<String> = {
        let candidate = backends_dir.join(&entry.id).join("backend.toml");
        if candidate.exists() {
            crate::stt_models::backends::manifest::Manifest::load(&backends_dir.join(&entry.id))
                .ok()
                .map(|m| m.backend.version)
        } else {
            None
        }
    };

    let Some(from_version) = installed_version else {
        return Err(Box::new(super::registry_error(
            StatusCode::NOT_FOUND,
            "not_installed",
        )));
    };

    Ok(VersionLookup {
        entry,
        from_version,
    })
}

/// Phase 4 — Select a compatible asset for the current host.
///
/// Returns `422` if no asset matches; the caller's [`InflightMarker`] cleans up
/// the inflight set.
fn select_update_compat(
    entry: &crate::registry::index_schema::IndexBackend,
) -> Result<crate::registry::compat::Selection, ErrResp> {
    use crate::registry::{compat, host_detect};

    let host = host_detect::detect();
    let sel = compat::select(&host, entry);

    if compat::to_selected_asset(entry, &sel).is_none() {
        return Err(Box::new(super::registry_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "incompatible",
        )));
    }

    Ok(sel)
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `POST /registry/backends/update` — re-run install if registry has a newer version.
pub(crate) async fn update_registry_backend(
    State(s): State<AppState>,
    body: Option<axum::Json<UpdateBody>>,
) -> impl IntoResponse {
    use super_stt_shared::registry::UpdateResponse;

    // Phase 1: parse body.
    let body = match parse_update_body(body) {
        Ok(b) => b,
        Err(r) => return *r,
    };

    // Phase 2: conflict guard. `marker` removes `body.source` from the inflight
    // set if dropped, so the fallible phases and the no-op early return below
    // just `return` and cleanup is automatic; the happy path defuses it before
    // spawning.
    let marker = match acquire_update_inflight(&s, &body.source) {
        Ok(m) => m,
        Err(r) => return *r,
    };

    // Phase 3: look up registry entry + installed version.
    let VersionLookup {
        entry,
        from_version,
    } = match lookup_versions(&s, &body.source).await {
        Ok(v) => v,
        Err(r) => return *r,
    };

    // No-op unless the registry offers a strictly-newer semver. String equality
    // treated any different string as an update, so an older or reformatted
    // registry version happily "updated" into a downgrade (Tier 1 #31); the
    // shared semver check refuses anything not strictly newer.
    if !super_stt_registry_types::version::update_available(&from_version, &entry.version) {
        // `marker` drops here → removes the inflight entry.
        let resp = UpdateResponse {
            install_id: None,
            from_version: from_version.clone(),
            to_version: from_version,
            noop: true,
        };
        return (
            StatusCode::OK,
            [("content-type", "application/json")],
            serde_json::to_string(&resp).unwrap_or_default(),
        )
            .into_response();
    }

    // Phase 4: compat selection.
    let sel = match select_update_compat(&entry) {
        Ok(s) => s,
        Err(r) => return *r,
    };

    // Phase 5: spawn background pipeline and return 202.
    let install_id = format!("ins_{}", ulid::Ulid::new());
    let to_version = entry.version.clone();

    // Spawn background install (same pipeline as install handler).
    // (source already marked inflight by the marker above)
    spawn_install_pipeline(
        Arc::clone(&s.daemon),
        Arc::clone(&s.install_inflight),
        entry,
        sel,
        install_id.clone(),
        body.source.clone(),
        None, // update never uses local_src
    );
    // The spawned pipeline's own `InflightGuard` now owns the marker's removal.
    marker.defuse();

    let resp = UpdateResponse {
        install_id: Some(install_id),
        from_version,
        to_version,
        noop: false,
    };
    (
        StatusCode::ACCEPTED,
        [("content-type", "application/json")],
        serde_json::to_string(&resp).unwrap_or_default(),
    )
        .into_response()
}
