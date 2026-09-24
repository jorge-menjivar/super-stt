// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope, RegistryError};
use axum::Json;
use axum::extract::State;
use axum::response::Response;
use super_engine_daemon::registry::endpoints::{self, InstallBody};
use super_stt_shared::registry::{InstallAccepted, InstallRequest};

/// `POST /registry/backend/install` — kick off a background install.
#[utoipa::path(
    post,
    path = "/registry/backend/install",
    tag = "registry",
    summary = "Install a backend",
    description = "\
Installs a backend from the registry, from a git-forge repository, or from a directory \
already staged on this machine — send exactly one of `source`, `repo_url`, or \
`local_path`. A `repo_url` install may also name its `forge`; without one the daemon \
picks the forge serving the URL's host.

The daemon picks the release asset matching this host's architecture and accelerators, \
and answers `202` as soon as that choice is made: the download, verification and \
install run in the background. Follow them on the `registry_install` event topic, keyed \
by the returned `install_id`. A `warning` of `unverified_source` means the bytes came \
from somewhere the registry does not vouch for — a custom repo or a local directory — \
and is worth showing the user.

Installing neither selects the backend nor loads a model; do that through \
`POST /pipeline/{stage}` and `POST /pipeline/{stage}/model`. Upgrading one already \
installed is `POST /registry/backend/update`, which runs this same pipeline.",
    request_body = InstallRequest,
    security(("session_token" = ["settings"])),
    responses(
        (status = 202, description = "Accepted; the install runs in the background.", body = InstallAccepted),
        (status = 400, description = "Not exactly one of `source`, `repo_url`, `local_path` (`bad_request`), a `repo_url` that is not `<host>/<owner>/<repo>` (`bad_repo_url`), a `repo_url` on a host no forge adapter serves and no `forge` to pick one (`unsupported_forge`), or a relative `local_path` (`bad_local_path`).", body = RegistryError),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No catalog entry for that `source`, or the repo, release, directory or manifest does not exist (`not_found`).", body = RegistryError),
        (status = 409, description = "An install for this backend is already in flight (`install_in_progress`).", body = RegistryError),
        (status = 422, description = "No asset matches this host (`incompatible`), or the manifest was rejected (`manifest_invalid`, `manifest_too_large`, `asset_missing`, `source_mismatch`).", body = RegistryError),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 502, description = "The forge could not be reached (`forge_unavailable`).", body = RegistryError),
        (status = 503, description = "The catalog could not be fetched and nothing is cached (`registry_unavailable`).", body = RegistryError),
    ),
)]
pub(crate) async fn install_registry_backend(
    State(s): State<AppState>,
    body: Option<Json<InstallBody>>,
) -> Response {
    endpoints::install(&s.registry, body).await
}
