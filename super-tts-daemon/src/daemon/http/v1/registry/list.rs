// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope, RegistryError};
use axum::extract::{Query, State};
use axum::response::Response;
use super_engine_daemon::registry::endpoints::{self, ListQuery};
use super_tts_shared::registry::RegistryListResponse;

/// `GET /registry/backend/list` — list installable backends from the registry.
#[utoipa::path(
    get,
    path = "/registry/backend/list",
    tag = "registry",
    summary = "Browse the published backend catalog",
    description = "\
Every backend published to the registry, with the models each serves and whether this \
machine can run it. `compatibility.compatible` is decided against the host's actual \
accelerators and this Super TTS's version, so the list reflects what is installable here \
rather than what exists in general; pass `include_incompatible=true` to see the rest, each \
with a `compatibility.reason` worth showing the user.

`needs_client_update` separates the two ways a backend can be blocked: a host that lacks \
the right GPU will never run it, but a Super TTS one version behind is something the user \
can fix in a minute. Those are listed even without `include_incompatible`; surface them \
differently.

`update_available` is the daemon's answer rather than the client's arithmetic: it \
compares the installed `backend.toml` on disk against what the index offers, by \
`source`, and refuses a downgrade. An entry with no `installed_version` is an install, \
not an update.

This is what is *available*; `GET /backend/list` is what is installed. The index is \
cached and re-fetched on a schedule — `POST /registry/backend/refresh` forces that \
now.",
    params(ListQuery),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The catalog.", body = RegistryListResponse),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "The catalog could not be fetched and nothing is cached (`registry_unavailable`). Retry, or force a fetch with `POST /registry/backend/refresh`.", body = RegistryError),
    ),
)]
pub(crate) async fn list_registry_backends(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Response {
    endpoints::list(&s.registry, &q).await
}
