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

    // URL-decode the source param (e.g. "github.com%2Fjorge-menjivar%2Fsuper-stt").
    let source = urlencoding::decode(&source_encoded)
        .map_or_else(|_| source_encoded.clone(), std::borrow::Cow::into_owned);

    // Look up the installed backend directory from the in-memory catalog.
    let install_dir = {
        let backends = s.daemon.backends.read().await;
        backends
            .iter()
            .find(|b| b.source == source)
            .map(|b| b.dir.clone())
    };

    let Some(dir) = install_dir else {
        return (
            StatusCode::NOT_FOUND,
            [("content-type", "application/json")],
            serde_json::json!({"error": "not_found"}).to_string(),
        )
            .into_response();
    };

    // Check whether this is the active backend before removing.
    let was_active = {
        let active = s.daemon.active_backend.read().await;
        active.as_deref() == dir.file_name().and_then(|n| n.to_str())
    };

    // Remove from disk.
    if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
        warn!("Failed to remove backend directory {}: {e}", dir.display());
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("content-type", "application/json")],
            serde_json::json!({"error": "remove_failed", "message": e.to_string()}).to_string(),
        )
            .into_response();
    }

    // Clear active backend if this was the active one.
    if was_active {
        let mut active = s.daemon.active_backend.write().await;
        *active = None;
        drop(active);
        let mut config = s.daemon.config.write().await;
        config.transcription.active_backend = None;
        drop(config);
        if let Err(e) = s.daemon.persist_config().await {
            warn!("Failed to persist config after uninstall: {e}");
        }
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
