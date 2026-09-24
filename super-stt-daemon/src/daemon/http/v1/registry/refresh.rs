// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope, RegistryError};
use axum::extract::State;
use axum::response::Response;
use super_engine_daemon::registry::endpoints;
use super_stt_shared::registry::RefreshResponse;

/// `POST /registry/backend/refresh` — force-refetch the registry index.
#[utoipa::path(
    post,
    path = "/registry/backend/refresh",
    tag = "registry",
    summary = "Re-fetch the backend catalog",
    description = "\
Pulls the published index again rather than serving what is cached, and reports how \
many backends it now holds. Use it after a backend is published, or to clear an \
`index_stale` marker a listing came back with.

`GET /registry/backend/list` refreshes on its own schedule; this forces it now. \
Idempotent, and concurrent calls coalesce into a single fetch, so a refresh button \
cannot start a stampede. The outcome is also published on the `registry_install` event \
topic, which is how a second client learns the catalog moved.",
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Refreshed.", body = RefreshResponse),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "The registry could not be reached (`registry_unavailable`).", body = RegistryError),
    ),
)]
pub(crate) async fn refresh_registry(State(s): State<AppState>) -> Response {
    endpoints::refresh(&s.registry).await
}
