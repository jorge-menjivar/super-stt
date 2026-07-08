// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use log::{info, warn};
use serde::Deserialize;

pub(crate) async fn list_backends(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "list_backends", None).await
}

pub(crate) async fn uninstall_backend(
    State(s): State<AppState>,
    AxumPath(source_encoded): AxumPath<String>,
) -> impl IntoResponse {
    use super_stt_shared::registry::UninstallResponse;
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
    };
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string(&resp).unwrap_or_default(),
    )
        .into_response()
}

pub(crate) async fn get_active_backend(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "get_active_backend", None).await
}

#[derive(Deserialize)]
pub(crate) struct SetActiveBackendBody {
    pub(crate) source: String,
}

pub(crate) async fn set_active_backend(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetActiveBackendBody>,
) -> impl IntoResponse {
    dispatch_command(
        &s.daemon,
        "set_active_backend",
        Some(serde_json::json!({ "source": body.source })),
    )
    .await
}

pub(crate) async fn clear_active_backend(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "clear_active_backend", None).await
}

pub(crate) async fn get_gpu_info(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "get_gpu_info", None).await
}
