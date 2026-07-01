// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use super_stt_shared::registry::events::RegistryEvent;

/// `POST /registry/backends/refresh` — force-refetch the registry index.
pub(crate) async fn refresh_registry(State(s): State<AppState>) -> impl IntoResponse {
    if let Ok(index) = s.registry_client.refresh().await {
        let payload = serde_json::to_value(RegistryEvent::RefreshCompleted {
            generated_at: index.generated_at.clone(),
            backend_count: index.backends.len(),
        })
        .unwrap_or_default();
        s.daemon.events.publish_registry_install(payload);

        let body = serde_json::json!({
            "schema_version": index.schema_version,
            "generated_at": index.generated_at,
            "backend_count": index.backends.len(),
        });
        (
            StatusCode::OK,
            [("content-type", "application/json")],
            body.to_string(),
        )
            .into_response()
    } else {
        let payload = serde_json::to_value(RegistryEvent::RefreshFailed {
            error: "registry_unavailable".to_string(),
        })
        .unwrap_or_default();
        s.daemon.events.publish_registry_install(payload);

        super::registry_error(StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
    }
}
