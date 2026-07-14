// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::pipeline::{InflightMarker, spawn_install_pipeline};

/// Request body for `POST /registry/backends/install`.
#[derive(Deserialize)]
pub(crate) struct InstallBody {
    pub(crate) source: Option<String>,
    pub(crate) repo_url: Option<String>,
    pub(crate) local_path: Option<String>,
    pub(crate) forge: Option<super_stt_registry_types::forge::Forge>,
}

/// Map a [`custom_repo::ResolveError`] to a synchronous HTTP status + body
/// `error` token. Mirrors the failure-modes table in
/// `docs/protocol/endpoints/v1/registry/install.md`.
fn custom_repo_error_response(
    e: &crate::registry::custom_repo::ResolveError,
) -> (StatusCode, &'static str) {
    use crate::registry::custom_repo::ResolveError;
    match e {
        ResolveError::BadRepoUrl(_) => (StatusCode::BAD_REQUEST, "bad_repo_url"),
        ResolveError::ManifestTooLarge => (StatusCode::UNPROCESSABLE_ENTITY, "manifest_too_large"),
        ResolveError::NotUtf8(_)
        | ResolveError::Manifest(_)
        | ResolveError::MissingWasmAsset
        | ResolveError::MissingSubprocessAssets
        | ResolveError::UnsafeComponent { .. } => {
            (StatusCode::UNPROCESSABLE_ENTITY, "manifest_invalid")
        }
        ResolveError::SourceSpoof { .. } => (StatusCode::UNPROCESSABLE_ENTITY, "source_mismatch"),
        ResolveError::AssetMissing(_) => (StatusCode::UNPROCESSABLE_ENTITY, "asset_missing"),
        ResolveError::Forge(err) => {
            // 404 from the forge means the repo, release, or backend.toml at the
            // tag is missing — surface as not_found rather than a generic 502.
            if err.http_status() == Some(reqwest::StatusCode::NOT_FOUND) {
                (StatusCode::NOT_FOUND, "not_found")
            } else {
                (StatusCode::BAD_GATEWAY, "forge_unavailable")
            }
        }
    }
}

/// Map a [`local_dir::ResolveError`] to a synchronous HTTP status + body
/// `error` token. Mirrors the failure-modes table in
/// `docs/protocol/endpoints/v1/registry/install.md`.
fn local_dir_error_response(
    e: &crate::registry::local_dir::ResolveError,
) -> (StatusCode, &'static str) {
    use crate::registry::local_dir::ResolveError;
    match e {
        ResolveError::NotAbsolute(_) => (StatusCode::BAD_REQUEST, "bad_local_path"),
        ResolveError::NotFound(_) | ResolveError::NoManifest(_) => {
            (StatusCode::NOT_FOUND, "not_found")
        }
        ResolveError::NotADirectory(_) => (StatusCode::UNPROCESSABLE_ENTITY, "bad_local_path"),
        ResolveError::Manifest(_) | ResolveError::UnsafeId(_) => {
            (StatusCode::UNPROCESSABLE_ENTITY, "manifest_invalid")
        }
    }
}

// ---------------------------------------------------------------------------
// Phase helpers
// ---------------------------------------------------------------------------

// Convenience alias: helpers return boxed responses so the `Err` variant
// stays pointer-sized and does not trip `clippy::result_large_err`.
type ErrResp = Box<axum::response::Response>;

/// Phase 1 — Parse and validate the request body.
///
/// Returns `(body, source_key)` on success, or an HTTP error response on
/// failure. `source_key` is whichever of `source`, `repo_url`, or
/// `local_path` was provided.
fn parse_install_body(
    raw: Option<axum::Json<InstallBody>>,
) -> Result<(InstallBody, String), ErrResp> {
    let Some(axum::Json(body)) = raw else {
        return Err(Box::new(super::registry_error(
            StatusCode::BAD_REQUEST,
            "missing_body",
        )));
    };

    // Validate exactly one of source / repo_url / local_path is present.
    let provided = [
        body.source.as_deref(),
        body.repo_url.as_deref(),
        body.local_path.as_deref(),
    ]
    .iter()
    .filter(|x| x.is_some())
    .count();
    if provided != 1 {
        return Err(Box::new(super::registry_error_msg(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "provide exactly one of source, repo_url, local_path",
        )));
    }

    let source_key = body
        .source
        .clone()
        .or_else(|| body.repo_url.clone())
        .or_else(|| body.local_path.clone())
        .unwrap();

    Ok((body, source_key))
}

/// Phase 2 — Acquire the inflight marker (conflict check + insert).
///
/// On conflict returns a `409` response. On success inserts `source_key` into
/// the set and returns an [`InflightMarker`] whose `Drop` removes it again, so
/// the later synchronous phases can bail with a bare `return` and still clean
/// up. The happy path calls [`InflightMarker::defuse`] to hand that duty to the
/// spawned pipeline.
fn acquire_install_inflight(s: &AppState, source_key: &str) -> Result<InflightMarker, ErrResp> {
    InflightMarker::acquire(Arc::clone(&s.install_inflight), source_key.to_owned()).ok_or_else(
        || {
            Box::new(super::registry_error(
                StatusCode::CONFLICT,
                "install_in_progress",
            ))
        },
    )
}

/// Phase 3 — Resolve the registry entry from whichever of
/// `source` / `repo_url` / `local_path` was supplied.
///
/// On any error an HTTP error response is returned; the caller's
/// [`InflightMarker`] cleans up the inflight set.
async fn resolve_install_entry(
    s: &AppState,
    body: &InstallBody,
) -> Result<crate::registry::index_schema::IndexBackend, ErrResp> {
    if let Some(ref src) = body.source {
        let Ok(index) = s.registry_client.get().await else {
            return Err(Box::new(super::registry_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "registry_unavailable",
            )));
        };
        let Some(found) = index.backends.iter().find(|b| &b.source == src).cloned() else {
            return Err(Box::new(super::registry_error(
                StatusCode::NOT_FOUND,
                "not_found",
            )));
        };
        Ok(found)
    } else if let Some(ref repo_url) = body.repo_url {
        let Some(forge) = body.forge else {
            return Err(Box::new(super::registry_error_msg(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "custom-repo install requires `forge`",
            )));
        };
        let client = super_stt_forge::client(forge);
        match crate::registry::custom_repo::resolve(client.as_ref(), repo_url).await {
            Ok(entry) => Ok(entry),
            Err(e) => {
                let (status, error) = custom_repo_error_response(&e);
                Err(Box::new(super::registry_error_msg(
                    status,
                    error,
                    &e.to_string(),
                )))
            }
        }
    } else {
        let path = body
            .local_path
            .as_deref()
            .map_or_else(|| Path::new(""), Path::new);
        match crate::registry::local_dir::resolve(path) {
            Ok(entry) => Ok(entry),
            Err(e) => {
                let (status, error) = local_dir_error_response(&e);
                Err(Box::new(super::registry_error_msg(
                    status,
                    error,
                    &e.to_string(),
                )))
            }
        }
    }
}

/// Phase 4 — Select a compatible asset for the current host.
///
/// The local-import path produces its own "selected asset" — an empty,
/// `accel = "local"` marker that documents how the bytes landed on disk.
/// The registry / custom-repo paths route through `compat::select`.
///
/// On incompatibility a `422` is returned; the caller's [`InflightMarker`]
/// cleans up the inflight set.
fn select_install_compat(
    entry: &crate::registry::index_schema::IndexBackend,
    local_src: Option<&PathBuf>,
) -> Result<
    (
        crate::registry::compat::Selection,
        super_stt_shared::registry::SelectedAsset,
    ),
    ErrResp,
> {
    use crate::registry::{compat, host_detect};

    if local_src.is_some() {
        // Local-import path: placeholder selection; run_local ignores it.
        let asset = super_stt_shared::registry::SelectedAsset {
            target: String::new(),
            accel: "local".into(),
            cuda_major: None,
            cuda_sm: None,
            cudnn: false,
        };
        return Ok((compat::Selection::Wasm, asset));
    }

    let host = host_detect::detect();
    let sel = compat::select(&host, entry);
    let Some(asset) = compat::to_selected_asset(entry, &sel) else {
        return Err(Box::new(super::registry_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "incompatible",
        )));
    };
    Ok((sel, asset))
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `POST /registry/backends/install` — kick off a background install.
pub(crate) async fn install_registry_backend(
    State(s): State<AppState>,
    body: Option<axum::Json<InstallBody>>,
) -> impl IntoResponse {
    // Phase 1: parse + validate.
    let (body, source_key) = match parse_install_body(body) {
        Ok(v) => v,
        Err(r) => return *r,
    };

    // Phase 2: conflict guard. `marker` removes `source_key` from the inflight
    // set if dropped, so the fallible phases below just `return` on error and
    // cleanup is automatic; the happy path defuses it before spawning.
    let marker = match acquire_install_inflight(&s, &source_key) {
        Ok(m) => m,
        Err(r) => return *r,
    };

    // Phase 3: resolve registry entry.
    let local_src: Option<PathBuf> = body.local_path.as_deref().map(PathBuf::from);
    let entry = match resolve_install_entry(&s, &body).await {
        Ok(e) => e,
        Err(r) => return *r,
    };

    // Phase 4: compat selection.
    let (sel, selected_asset_resp) = match select_install_compat(&entry, local_src.as_ref()) {
        Ok(v) => v,
        Err(r) => return *r,
    };

    // Phase 5: shape response and spawn background pipeline.
    let install_id = format!("ins_{}", ulid::Ulid::new());
    let warning = if body.repo_url.is_some() || local_src.is_some() {
        Some("unverified_source".to_string())
    } else {
        None
    };

    let resp_body = super_stt_shared::registry::InstallAccepted {
        install_id: install_id.clone(),
        source: entry.source.clone(),
        version: entry.version.clone(),
        selected_asset: selected_asset_resp,
        warning,
    };

    // Spawn background install task. Events and inflight cleanup key off
    // `source_key` (what the client sent: registry source, repo URL, or
    // local path) so the app — which tracks an install under what it sent —
    // and the daemon stay in sync. The canonical `entry.source` is reported
    // separately in the `InstallAccepted` response and discovered backend
    // metadata once the install finishes.
    spawn_install_pipeline(
        Arc::clone(&s.daemon),
        Arc::clone(&s.install_inflight),
        entry,
        sel,
        install_id,
        source_key,
        local_src,
    );
    // The spawned pipeline's own `InflightGuard` now owns the marker's removal.
    marker.defuse();

    (
        StatusCode::ACCEPTED,
        [("content-type", "application/json")],
        serde_json::to_string(&resp_body).unwrap_or_default(),
    )
        .into_response()
}
