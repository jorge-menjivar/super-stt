// SPDX-License-Identifier: GPL-3.0-only
use super::pipeline::{InflightMarker, spawn_install_pipeline};
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope, RegistryError};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use std::sync::Arc;
use super_stt_shared::registry::{UpdateRequest, UpdateResponse};

/// Request body for `POST /registry/backend/update`.
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

    // Matched by `source`, the same key `GET /registry/backend/list` uses to
    // decide `update_available` — and the same helper, so the two never drift.
    let installed_version: Option<String> = {
        let backends = s.daemon.backends.read().await;
        super::list::installed_version_for_source(&backends, &entry.source)
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
        // Same as the install path: say why, rather than making the client
        // re-derive it from a listing it may not have refreshed.
        return Err(Box::new(super::registry_error_msg(
            StatusCode::UNPROCESSABLE_ENTITY,
            "incompatible",
            sel.reason().unwrap_or("no compatible asset for this host"),
        )));
    }

    Ok(sel)
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `POST /registry/backend/update` — re-run install if registry has a newer version.
#[utoipa::path(
    post,
    path = "/registry/backend/update",
    tag = "registry",
    summary = "Update an installed backend",
    description = "\
Upgrades an installed backend to the newest version the catalog offers for this \
host. Answers with the versions moved between; `noop` is `true` when the installed \
version was already current, which is a success rather than an error.

Like install, the download runs in the background \u{2014} follow `install_id` on the \
`registry_install` topic when one is returned.",
    request_body = UpdateRequest,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Up to date already, or the upgrade was accepted.", body = UpdateResponse),
        (status = 404, description = "No installed backend has that `source` (`not_found`).", body = RegistryError),
        (status = 409, description = "An install is already in flight, or a recording is running (`backend_busy`).", body = RegistryError),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn update_registry_backend(
    State(s): State<AppState>,
    body: Option<axum::Json<UpdateBody>>,
) -> impl IntoResponse {
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
