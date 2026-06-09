// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use parking_lot::RwLock as ParkingRwLock;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use super_stt_shared::registry::events::RegistryEvent;

/// Request body for `POST /registry/backends/install`.
#[derive(Deserialize)]
pub(crate) struct InstallBody {
    pub(crate) source: Option<String>,
    pub(crate) repo_url: Option<String>,
    pub(crate) local_path: Option<String>,
}

/// Ensures a spawned install/update task always reaches a terminal state. On
/// the happy path the task calls [`InflightGuard::disarm`] after emitting its
/// own `Completed`/`Failed` event. If the task instead unwinds (a panic in the
/// pipeline) before that, `Drop` clears the inflight marker — so retries are not
/// blocked by a stale `409` — and emits a `Failed` event so the client's
/// progress UI does not spin forever.
pub(super) struct InflightGuard {
    inflight: Arc<ParkingRwLock<HashSet<String>>>,
    events: Arc<crate::daemon::events::EventBus>,
    install_id: String,
    source: String,
    armed: bool,
}

impl InflightGuard {
    pub(super) fn new(
        inflight: Arc<ParkingRwLock<HashSet<String>>>,
        events: Arc<crate::daemon::events::EventBus>,
        install_id: String,
        source: String,
    ) -> Self {
        Self {
            inflight,
            events,
            install_id,
            source,
            armed: true,
        }
    }

    /// Normal completion: clear the inflight marker and suppress the
    /// unwind-path `Failed` event.
    pub(super) fn disarm(mut self) {
        self.inflight.write().remove(&self.source);
        self.armed = false;
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        use super_stt_shared::registry::events::{InstallError, InstallPhase};
        if !self.armed {
            return;
        }
        self.inflight.write().remove(&self.source);
        let ev = RegistryEvent::Failed {
            install_id: self.install_id.clone(),
            source: self.source.clone(),
            phase: InstallPhase::Installing,
            error: InstallError::InstallIoError,
        };
        self.events
            .publish_registry_install(serde_json::to_value(ev).unwrap_or_default());
    }
}

/// Map a [`custom_repo::ResolveError`] to a synchronous HTTP status + body
/// `error` token. Mirrors the failure-modes table in
/// `docs/protocol/endpoints/v1/registry/install.md`.
fn custom_repo_error_response(
    e: &crate::registry::custom_repo::ResolveError,
) -> (StatusCode, &'static str) {
    use crate::registry::custom_repo::ResolveError;
    use crate::registry::github::GitHubError;
    match e {
        ResolveError::BadRepoUrl(_) => (StatusCode::BAD_REQUEST, "bad_repo_url"),
        ResolveError::ManifestTooLarge => (StatusCode::UNPROCESSABLE_ENTITY, "manifest_too_large"),
        ResolveError::NotUtf8(_)
        | ResolveError::Toml(_)
        | ResolveError::MissingWasmAsset
        | ResolveError::MissingSubprocessAssets
        | ResolveError::UnknownKind(_)
        | ResolveError::UnsafeComponent { .. } => {
            (StatusCode::UNPROCESSABLE_ENTITY, "manifest_invalid")
        }
        ResolveError::SourceSpoof { .. } => (StatusCode::UNPROCESSABLE_ENTITY, "source_mismatch"),
        ResolveError::AssetMissing(_) => (StatusCode::UNPROCESSABLE_ENTITY, "asset_missing"),
        ResolveError::GitHub(GitHubError::Http(err)) => {
            // 404 from GitHub means the repo, release, or backend.toml at the
            // tag is missing — surface as not_found rather than a generic 502.
            if err.status() == Some(reqwest::StatusCode::NOT_FOUND) {
                (StatusCode::NOT_FOUND, "not_found")
            } else {
                (StatusCode::BAD_GATEWAY, "github_unavailable")
            }
        }
        ResolveError::GitHub(_) => (StatusCode::BAD_GATEWAY, "github_unavailable"),
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
        return Err(Box::new(
            (
                StatusCode::BAD_REQUEST,
                [("content-type", "application/json")],
                serde_json::json!({"error": "missing_body"}).to_string(),
            )
                .into_response(),
        ));
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
        return Err(Box::new((
            StatusCode::BAD_REQUEST,
            [("content-type", "application/json")],
            serde_json::json!({"error": "bad_request", "message": "provide exactly one of source, repo_url, local_path"}).to_string(),
        )
            .into_response()));
    }

    let source_key = body
        .source
        .clone()
        .or_else(|| body.repo_url.clone())
        .or_else(|| body.local_path.clone())
        .unwrap();

    Ok((body, source_key))
}

/// Phase 2 — Acquire the inflight guard (conflict check + insert).
///
/// On conflict returns a `409` response. On success inserts `source_key`
/// into the set and returns `()`.
fn acquire_install_inflight(s: &AppState, source_key: &str) -> Result<(), ErrResp> {
    let mut guard = s.install_inflight.write();
    if guard.contains(source_key) {
        return Err(Box::new(
            (
                StatusCode::CONFLICT,
                [("content-type", "application/json")],
                serde_json::json!({"error": "install_in_progress"}).to_string(),
            )
                .into_response(),
        ));
    }
    guard.insert(source_key.to_owned());
    Ok(())
}

/// Phase 3 — Resolve the registry entry from whichever of
/// `source` / `repo_url` / `local_path` was supplied.
///
/// On any error the inflight set is cleaned up and an HTTP error response is
/// returned.
async fn resolve_install_entry(
    s: &AppState,
    body: &InstallBody,
    source_key: &str,
) -> Result<crate::registry::index_schema::IndexBackend, ErrResp> {
    if let Some(ref src) = body.source {
        let Ok(index) = s.registry_client.get().await else {
            s.install_inflight.write().remove(source_key);
            return Err(Box::new(
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("content-type", "application/json")],
                    serde_json::json!({"error": "registry_unavailable"}).to_string(),
                )
                    .into_response(),
            ));
        };
        let Some(found) = index.backends.iter().find(|b| &b.source == src).cloned() else {
            s.install_inflight.write().remove(source_key);
            return Err(Box::new(
                (
                    StatusCode::NOT_FOUND,
                    [("content-type", "application/json")],
                    serde_json::json!({"error": "not_found"}).to_string(),
                )
                    .into_response(),
            ));
        };
        Ok(found)
    } else if let Some(ref repo_url) = body.repo_url {
        let gh = crate::registry::github::GitHub::from_env();
        match crate::registry::custom_repo::resolve(&gh, repo_url).await {
            Ok(entry) => Ok(entry),
            Err(e) => {
                s.install_inflight.write().remove(source_key);
                let (status, error) = custom_repo_error_response(&e);
                Err(Box::new(
                    (
                        status,
                        [("content-type", "application/json")],
                        serde_json::json!({"error": error, "message": e.to_string()}).to_string(),
                    )
                        .into_response(),
                ))
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
                s.install_inflight.write().remove(source_key);
                let (status, error) = local_dir_error_response(&e);
                Err(Box::new(
                    (
                        status,
                        [("content-type", "application/json")],
                        serde_json::json!({"error": error, "message": e.to_string()}).to_string(),
                    )
                        .into_response(),
                ))
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
/// On incompatibility the inflight set is cleaned up and a `422` is returned.
fn select_install_compat(
    s: &AppState,
    entry: &crate::registry::index_schema::IndexBackend,
    local_src: Option<&PathBuf>,
    source_key: &str,
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
    let prefs = compat::Prefs::default();
    let sel = compat::select(&host, entry, &prefs);
    let Some(asset) = compat::to_selected_asset(entry, &sel) else {
        s.install_inflight.write().remove(source_key);
        return Err(Box::new(
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                [("content-type", "application/json")],
                serde_json::json!({"error": "incompatible"}).to_string(),
            )
                .into_response(),
        ));
    };
    Ok((sel, asset))
}

/// Shared background pipeline spawner used by both the install and update
/// handlers. The caller has already inserted `source_bg` into the inflight
/// set; this function takes ownership of that responsibility via
/// [`InflightGuard`].
///
/// `local_src` is `Some` only for the install handler's local-path branch;
/// the update handler always passes `None`.
pub(super) fn spawn_install_pipeline(
    daemon: Arc<crate::daemon::types::SuperSTTDaemon>,
    inflight: Arc<ParkingRwLock<HashSet<String>>>,
    entry: crate::registry::index_schema::IndexBackend,
    sel: crate::registry::compat::Selection,
    install_id: String,
    source: String,
    local_src: Option<PathBuf>,
) {
    tokio::spawn(async move {
        // Always reach a terminal state, even if the pipeline panics.
        let guard = InflightGuard::new(
            inflight,
            Arc::clone(&daemon.events),
            install_id.clone(),
            source.clone(),
        );
        let backends_dir = {
            let c = daemon.config.read().await;
            c.transcription.backends_dir.clone().map_or_else(
                crate::stt_models::backends::default_backends_dir,
                PathBuf::from,
            )
        };
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("super-stt/install");

        let events = Arc::clone(&daemon.events);
        let install_id_ev = install_id.clone();
        let source_ev = source.clone();

        let pipeline = crate::registry::install::Pipeline {
            backends_dir,
            cache_dir,
            http: reqwest::Client::builder()
                .timeout(Duration::from_mins(5))
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()
                .unwrap_or_default(),
            on_progress: Arc::new(move |phase, bytes: Option<(u64, Option<u64>)>| {
                use super_stt_shared::registry::events::{InstallPhase, RegistryEvent};
                let (bytes_done, bytes_total) = bytes.map_or((None, None), |(d, t)| (Some(d), t));
                let ev = RegistryEvent::Progress {
                    install_id: install_id_ev.clone(),
                    source: source_ev.clone(),
                    phase: match phase {
                        InstallPhase::Resolving => InstallPhase::Resolving,
                        InstallPhase::Downloading => InstallPhase::Downloading,
                        InstallPhase::Verifying => InstallPhase::Verifying,
                        InstallPhase::Extracting => InstallPhase::Extracting,
                        InstallPhase::Installing => InstallPhase::Installing,
                        InstallPhase::Rescanning => InstallPhase::Rescanning,
                    },
                    bytes_done,
                    bytes_total,
                };
                events.publish_registry_install(serde_json::to_value(ev).unwrap_or_default());
            }),
        };

        let events2 = Arc::clone(&daemon.events);
        let outcome = if let Some(src_dir) = local_src.as_ref() {
            crate::registry::install::run_local(&pipeline, &entry, src_dir).await
        } else {
            crate::registry::install::run(&pipeline, &entry, &sel).await
        };
        match outcome {
            Ok(version) => {
                // Refresh the daemon's in-memory backend catalog.
                daemon.refresh_backends().await;

                let ev = RegistryEvent::Completed {
                    install_id: install_id.clone(),
                    source: source.clone(),
                    version,
                };
                events2.publish_registry_install(serde_json::to_value(ev).unwrap_or_default());
            }
            Err((phase, error)) => {
                let ev = RegistryEvent::Failed {
                    install_id: install_id.clone(),
                    source: source.clone(),
                    phase,
                    error,
                };
                events2.publish_registry_install(serde_json::to_value(ev).unwrap_or_default());
            }
        }

        guard.disarm();
    });
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

    // Phase 2: conflict guard.
    if let Err(r) = acquire_install_inflight(&s, &source_key) {
        return *r;
    }

    // Phase 3: resolve registry entry.
    let local_src: Option<PathBuf> = body.local_path.as_deref().map(PathBuf::from);
    let entry = match resolve_install_entry(&s, &body, &source_key).await {
        Ok(e) => e,
        Err(r) => return *r,
    };

    // Phase 4: compat selection.
    let (sel, selected_asset_resp) =
        match select_install_compat(&s, &entry, local_src.as_ref(), &source_key) {
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

    (
        StatusCode::ACCEPTED,
        [("content-type", "application/json")],
        serde_json::to_string(&resp_body).unwrap_or_default(),
    )
        .into_response()
}
