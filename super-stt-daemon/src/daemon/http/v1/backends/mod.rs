// SPDX-License-Identifier: GPL-3.0-only
//! `/backends` — the backends installed on this machine.
//!
//! Contract: `docs/protocol/endpoints/v1/backends.md`.
//!
//! The catalog and one backend's removal are here, at the paths they answer on;
//! the sub-resources each get their own module — [`options`] for
//! `/backends/{source}/options`, [`secrets`] for `/backends/{source}/secrets`,
//! [`model_language`] for `/backends/{source}/models/{model}/language`.
//!
//! Installing is not here: that is the registry's job, at
//! [`/registry/backends/install`](super::registry::install). This module is
//! about what is already on disk.
//!
//! One split runs through the whole family: every path but the secrets ones is
//! `settings`-scoped, and secrets are `secrets`-scoped. [`routes`] gathers the
//! former; [`secrets::routes`] is wired to its own guard in [`super`].
pub(crate) mod model_language;
pub(crate) mod options;
pub(crate) mod secrets;

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use crate::daemon::http::v1::wire::{BackendCatalog, FromDaemon};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope, RegistryError};
use crate::stt_models::backends::DiscoveredBackend;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use log::{info, warn};
use super_stt_shared::registry::UninstallResponse;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// Percent-decode a `{source}` path segment (e.g. `github.com%2Facme%2Fx`).
pub(crate) fn decode_source(raw: &str) -> String {
    urlencoding::decode(raw).map_or_else(|_| raw.to_string(), std::borrow::Cow::into_owned)
}

/// Clone the catalog entry for `source`, if installed.
pub(crate) async fn find_backend(s: &AppState, source: &str) -> Option<DiscoveredBackend> {
    s.daemon
        .backends
        .read()
        .await
        .iter()
        .find(|b| b.source == source)
        .cloned()
}

/// House-style JSON error envelope at a given status. `error_code` is the stable
/// machine-readable `snake_case` identifier clients switch on (per `transport.md`,
/// "present on every error"); it is also mirrored into `message` since these
/// backend endpoints carry no separate human-readable text (audit 2 Tier 2 #6).
pub(crate) fn json_error(code: StatusCode, error_code: &str) -> Response {
    json_error_msg(code, error_code, error_code)
}

/// [`json_error`] with a distinct human-readable `message` (the machine
/// identifier still rides in `error_code`).
pub(crate) fn json_error_msg(code: StatusCode, error_code: &str, message: &str) -> Response {
    (
        code,
        [("content-type", "application/json")],
        serde_json::json!({ "status": "error", "error_code": error_code, "message": message })
            .to_string(),
    )
        .into_response()
}

/// House-style JSON success response with status 200.
///
/// Generic over the body so each endpoint hands it the narrow type it publishes
/// in the `OpenAPI` document, rather than a `Value` that has forgotten its shape.
pub(crate) fn ok<T: serde::Serialize>(v: &T) -> Response {
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string(v).unwrap_or_else(|_| String::from(r#"{"status":"error"}"#)),
    )
        .into_response()
}

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

/// The `settings`-scoped `/backends` routes, gathered for the settings group.
///
/// [`secrets`] is deliberately absent: those paths carry their own scope and are
/// registered against their own guard in [`super`].
pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_backends))
        .routes(routes!(uninstall_backend))
        .merge(options::routes())
        .merge(model_language::routes())
}
