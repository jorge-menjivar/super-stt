// SPDX-License-Identifier: GPL-3.0-only
use super::install::spawn_install_pipeline;
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
        return Err(Box::new(
            (
                StatusCode::BAD_REQUEST,
                [("content-type", "application/json")],
                serde_json::json!({"error": "missing_body"}).to_string(),
            )
                .into_response(),
        ));
    };
    Ok(body)
}

/// Phase 2 — Acquire the inflight guard (conflict check + insert).
///
/// Single write-lock so the check+insert is atomic.
/// Returns `Err` with a `409` response when an update is already in progress.
fn acquire_update_inflight(s: &AppState, source: &str) -> Result<(), ErrResp> {
    if !s.install_inflight.write().insert(source.to_owned()) {
        return Err(Box::new(
            (
                StatusCode::CONFLICT,
                [("content-type", "application/json")],
                serde_json::json!({"error": "update_in_progress"}).to_string(),
            )
                .into_response(),
        ));
    }
    Ok(())
}

/// Outcome of the version-lookup phase.
struct VersionLookup {
    entry: crate::registry::index_schema::IndexBackend,
    from_version: String,
}

/// Phase 3 — Fetch the registry entry and determine the installed version.
///
/// Cleans up the inflight marker and returns an HTTP error on any failure:
/// registry unavailable, source not in index, not installed on disk.
async fn lookup_versions(s: &AppState, source: &str) -> Result<VersionLookup, ErrResp> {
    let backends_dir = {
        let c = s.daemon.config.read().await;
        c.transcription.backends_dir.clone().map_or_else(
            crate::stt_models::backends::default_backends_dir,
            PathBuf::from,
        )
    };

    let Ok(index) = s.registry_client.get().await else {
        s.install_inflight.write().remove(source);
        return Err(Box::new(
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [("content-type", "application/json")],
                serde_json::json!({"error": "registry_unavailable"}).to_string(),
            )
                .into_response(),
        ));
    };

    let Some(entry) = index.backends.iter().find(|b| b.source == source).cloned() else {
        s.install_inflight.write().remove(source);
        return Err(Box::new(
            (
                StatusCode::NOT_FOUND,
                [("content-type", "application/json")],
                serde_json::json!({"error": "not_found"}).to_string(),
            )
                .into_response(),
        ));
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
        s.install_inflight.write().remove(source);
        return Err(Box::new(
            (
                StatusCode::NOT_FOUND,
                [("content-type", "application/json")],
                serde_json::json!({"error": "not_installed"}).to_string(),
            )
                .into_response(),
        ));
    };

    Ok(VersionLookup {
        entry,
        from_version,
    })
}

/// Phase 4 — Select a compatible asset for the current host.
///
/// Cleans up the inflight marker and returns `422` if no asset matches.
fn select_update_compat(
    s: &AppState,
    entry: &crate::registry::index_schema::IndexBackend,
    source: &str,
) -> Result<crate::registry::compat::Selection, ErrResp> {
    use crate::registry::{compat, host_detect};

    let host = host_detect::detect();
    let prefs = compat::Prefs::default();
    let sel = compat::select(&host, entry, &prefs);

    if compat::to_selected_asset(entry, &sel).is_none() {
        s.install_inflight.write().remove(source);
        return Err(Box::new(
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                [("content-type", "application/json")],
                serde_json::json!({"error": "incompatible"}).to_string(),
            )
                .into_response(),
        ));
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

    // Phase 2: conflict guard.
    if let Err(r) = acquire_update_inflight(&s, &body.source) {
        return *r;
    }

    // Phase 3: look up registry entry + installed version.
    let VersionLookup {
        entry,
        from_version,
    } = match lookup_versions(&s, &body.source).await {
        Ok(v) => v,
        Err(r) => return *r,
    };

    // No-op if already at the latest version.
    if from_version == entry.version {
        s.install_inflight.write().remove(&body.source);
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
    let sel = match select_update_compat(&s, &entry, &body.source) {
        Ok(s) => s,
        Err(r) => return *r,
    };

    // Phase 5: spawn background pipeline and return 202.
    let install_id = format!("ins_{}", ulid::Ulid::new());
    let to_version = entry.version.clone();

    // Spawn background install (same pipeline as install handler).
    // (source already marked inflight by the guard above)
    spawn_install_pipeline(
        Arc::clone(&s.daemon),
        Arc::clone(&s.install_inflight),
        entry,
        sel,
        install_id.clone(),
        body.source.clone(),
        None, // update never uses local_src
    );

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
