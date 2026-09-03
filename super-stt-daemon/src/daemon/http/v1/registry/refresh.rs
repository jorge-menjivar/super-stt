// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope, RegistryError};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use super_stt_shared::registry::RefreshResponse;
use super_stt_shared::registry::events::RegistryEvent;

/// `POST /registry/backends/refresh` — force-refetch the registry index.
#[utoipa::path(
    post,
    path = "/registry/backends/refresh",
    tag = "registry",
    summary = "Re-fetch the backend catalog",
    description = "\
Pulls the published index again rather than serving what is cached, and reports how \
many backends it now holds. Use it after a backend is published, or to clear an \
`index_stale` flag.

`GET /registry/backends` refreshes on its own schedule; this forces it now.",
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Refreshed.", body = RefreshResponse),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "The catalog could not be fetched.", body = RegistryError),
    ),
)]
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
