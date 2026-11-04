// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope, RegistryError};
use axum::Json;
use axum::extract::State;
use axum::response::Response;
use super_engine_daemon::registry::endpoints::{self, UpdateBody};
use super_tts_shared::registry::{UpdateRequest, UpdateResponse};

/// `POST /registry/backend/update` — re-run install if registry has a newer version.
#[utoipa::path(
    post,
    path = "/registry/backend/update",
    tag = "registry",
    summary = "Update an installed backend",
    description = "\
Upgrades an installed backend to the newest version the catalog offers for this host. \
Answers with the versions moved between; `noop` is `true` when the installed version \
was already current, which is a success rather than an error and carries no \
`install_id`.

Only a strictly newer semver is an upgrade: a catalog that has gone backwards, or a \
version neither side can parse, is a no-op rather than a silent downgrade. \
`update_available` on `GET /registry/backend/list` is the same comparison, so a client \
can offer the button only where this will do something.

Like install, the work runs in the background — follow `install_id` on the \
`registry_install` topic when one is returned.",
    request_body = UpdateRequest,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Already current; nothing was installed.", body = UpdateResponse),
        (status = 202, description = "Accepted; the upgrade runs in the background.", body = UpdateResponse),
        (status = 400, description = "The request carried no body (`missing_body`).", body = RegistryError),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No catalog entry for that `source` (`not_found`), or nothing is installed under it (`not_installed`).", body = RegistryError),
        (status = 409, description = "An install or update for this backend is already in flight (`update_in_progress`).", body = RegistryError),
        (status = 422, description = "The newer release has no asset this host can run (`incompatible`).", body = RegistryError),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "The catalog could not be fetched and nothing is cached (`registry_unavailable`).", body = RegistryError),
    ),
)]
pub(crate) async fn update_registry_backend(
    State(s): State<AppState>,
    body: Option<Json<UpdateBody>>,
) -> Response {
    endpoints::update(&s.registry, body).await
}
