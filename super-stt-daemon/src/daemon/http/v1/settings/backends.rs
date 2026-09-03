// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use crate::daemon::http::v1::settings::wire::{BackendCatalog, FromDaemon, GpuInventory};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope, RegistryError};
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use log::{info, warn};
use super_stt_shared::registry::UninstallResponse;

#[utoipa::path(
    get,
    path = "/backends",
    tag = "backends",
    summary = "List installed backends",
    description = "\
Every backend installed on this machine, each with the models it serves, the options \
it exposes, and which of its secrets are configured. Secret *values* are never \
returned — only whether each is set.

This is the full catalog, roles included. `GET /models` is the narrower read a \
transcription-model picker wants; browsing what is *available to install* is \
`GET /registry/backends`.",
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The installed catalog.", body = BackendCatalog),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn list_backends(State(s): State<AppState>) -> Response {
    let resp = dispatch(&s.daemon, build_request("list_backends", None)).await;
    narrowed(resp, BackendCatalog::from_daemon)
}

#[utoipa::path(
    delete,
    path = "/backends/{source}",
    tag = "backends",
    summary = "Uninstall a backend",
    description = "\
Removes an installed backend and its files from disk.

If the backend was filling a pipeline stage, that stage is emptied first: any loaded \
model is unloaded and the selection cleared, so the daemon does not end up pointing \
at files that no longer exist. The response says which stages were affected.

Refused with `409 backend_busy` while a recording or realtime session is in flight \
— removing files out from under one would strand state it still depends on.",
    params(
        ("source" = String, Path,
         description = "The backend's `source`, percent-encoded — e.g. `github.com%2Facme%2Fwhisper`.",
         example = "github.com%2Facme%2Fwhisper"),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Removed.", body = UninstallResponse),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No installed backend claims that `source`.", body = RegistryError),
        (status = 409, description = "A recording or realtime session is in flight (`backend_busy`).", body = RegistryError),
        (status = 500, description = "The files could not be removed (`remove_failed`).", body = RegistryError),
    ),
)]
pub(crate) async fn uninstall_backend(
    State(s): State<AppState>,
    AxumPath(source_encoded): AxumPath<String>,
) -> Response {
    // Uninstall is the inverse of install, so it shares the registry error
    // envelope (`{ "error": <code> }`) rather than hand-rolling its own shape.
    use crate::daemon::http::v1::registry::{registry_error, registry_error_msg};

    // URL-decode the source param (e.g. "github.com%2Fjorge-menjivar%2Fsuper-stt").
    let source = urlencoding::decode(&source_encoded)
        .map_or_else(|_| source_encoded.clone(), std::borrow::Cow::into_owned);

    // Refuse to mutate the backend set mid-recording / mid-realtime — the same
    // guard the model/backend switch commands use. Removing a backend (and the
    // `refresh_backends` that follows) under an in-flight session would strand
    // state the session still depends on.
    if s.daemon.switch_guard().await.is_some() {
        return registry_error(StatusCode::CONFLICT, "backend_busy");
    }

    // Look up the installed backend directory from the in-memory catalog.
    let install_dir = {
        let backends = s.daemon.backends.read().await;
        backends
            .iter()
            .find(|b| b.source == source)
            .map(|b| b.dir.clone())
    };

    let Some(dir) = install_dir else {
        return registry_error(StatusCode::NOT_FOUND, "not_found");
    };

    // Check whether this is the active backend before removing.
    let was_active = {
        let active = s.daemon.active_backend.read().await;
        active.as_deref() == dir.file_name().and_then(|n| n.to_str())
    };

    // If this is the active backend, go fully idle *before* the files vanish:
    // unload the loaded model (frees device memory / the subprocess unit and
    // keeps `GET /status` consistent with `GET /active_backend`) and clear the
    // active-backend + preferred-model config. Previously the model was left
    // loaded and only `active_backend` was cleared.
    if was_active {
        s.daemon.handle_clear_active_backend().await;
    }
    // Stage 2 fills from the same catalog and was left behind: an uninstalled
    // backend's post-processor kept its subprocess running against deleted
    // files, and the config kept naming it. The selection is by `source`,
    // loaded or not — a stage pointed at a backend that no longer exists is
    // wrong either way.
    let was_post_processor = s.daemon.config.read().await.post_processor.source.as_str() == source;
    if was_post_processor {
        s.daemon.handle_clear_post_processor_backend().await;
    }

    // Remove from disk.
    if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
        warn!("Failed to remove backend directory {}: {e}", dir.display());
        return registry_error_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            "remove_failed",
            &e.to_string(),
        );
    }

    // Refresh in-memory discovery.
    s.daemon.refresh_backends().await;

    info!("Uninstalled backend {source} from {}", dir.display());

    let resp = UninstallResponse {
        uninstalled: true,
        was_active,
        was_post_processor,
    };
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string(&resp).unwrap_or_default(),
    )
        .into_response()
}

#[utoipa::path(
    get,
    path = "/gpu_info",
    tag = "settings",
    summary = "Inventory the host's GPUs",
    description = "\
What the daemon can see of this machine's accelerators: one entry per detected GPU \
with its memory, plus the host-wide driver and runtime versions that decide which \
backend builds will actually run here.

Detection is a live probe, so this reflects the machine now rather than a cached \
answer. A host with no GPU answers `200` with an empty list.",
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The detected GPUs and host toolchain versions.", body = GpuInventory),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn get_gpu_info(State(s): State<AppState>) -> Response {
    let resp = dispatch(&s.daemon, build_request("get_gpu_info", None)).await;
    narrowed(resp, GpuInventory::from_daemon)
}
