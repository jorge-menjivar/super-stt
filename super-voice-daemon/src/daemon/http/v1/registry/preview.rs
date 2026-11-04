// SPDX-License-Identifier: GPL-3.0-only
//! `POST /registry/backend/preview` — resolve an install source and describe
//! what it would install, without installing it.
//!
//! Same body as [`install`](super::install), same resolution, and the answer is
//! the catalog shape `GET /registry/backend/list` already returns, so a client
//! renders a preview with whatever it uses to render a Browse card. What it
//! does not do is write: no inflight marker, no background pipeline, nothing on
//! disk. Two calls in a row are the same as one.
//!
//! It exists because the install endpoint answers `202` the moment it has
//! chosen an asset, which is *after* the point of no return. A person pasting a
//! repository URL has no other way to see what is in it: the name, what it
//! runs as, what it can reach, whether this machine can run it at all.

use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope, RegistryError};
use axum::Json;
use axum::extract::State;
use axum::response::Response;
use super_engine_daemon::registry::endpoints::{self, InstallBody};
use super_stt_shared::registry::{InstallRequest, PreviewResponse};

/// `POST /registry/backend/preview` — describe what a source would install.
#[utoipa::path(
    post,
    path = "/registry/backend/preview",
    tag = "registry",
    summary = "Preview what a source would install",
    description = "\
Resolves `source`, `repo_url`, or `local_path` exactly as `/registry/backend/install` \
does, then answers with the backend it found instead of installing it. Send exactly \
one of the three.

The backend comes back in the same shape as a `/registry/backend/list` entry, \
`compatibility` included, so a client can show a preview with whatever it already \
uses for the catalog. `warning` carries `unverified_source` for the custom-repo and \
local-import routes, the same as the install response.

Nothing is written and nothing is downloaded beyond the manifest needed to answer, \
so this is safe to call before every install and safe to call twice.",
    request_body = InstallRequest,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The backend this source would install.", body = PreviewResponse),
        (status = 400, description = "Not exactly one of `source`, `repo_url`, `local_path` (`bad_request`), a `repo_url` that is not a `<host>/<owner>/<repo>` reference (`bad_repo_url`), or a `repo_url` on a host no forge adapter serves (`unsupported_forge`).", body = RegistryError),
        (status = 404, description = "No catalog entry for that `source`, or the repo has no published release (`not_found`).", body = RegistryError),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn preview_registry_backend(
    State(s): State<AppState>,
    body: Option<Json<InstallBody>>,
) -> Response {
    endpoints::preview(&s.registry, body).await
}
