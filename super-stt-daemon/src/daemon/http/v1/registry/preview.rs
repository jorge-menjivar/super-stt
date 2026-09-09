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
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use super_stt_shared::registry::{Compatibility, InstallRequest, PreviewResponse};

use super::install::InstallBody;

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
    body: Option<axum::Json<InstallBody>>,
) -> impl IntoResponse {
    use crate::registry::{compat, host_detect};

    let (body, _source_key) = match super::install::parse_install_body(body) {
        Ok(v) => v,
        Err(r) => return *r,
    };

    // Same resolution as an install, so a preview that succeeds and an install
    // that fails cannot disagree about what the source is.
    let entry = match super::install::resolve_install_entry(&s, &body).await {
        Ok(e) => e,
        Err(r) => return *r,
    };

    let compat_field = if body.local_path.is_some() {
        // A local import has no published asset to match against this host: the
        // operator staged the bytes, so the only build in play is the one on
        // disk. Mirrors the placeholder the install path uses.
        Compatibility {
            compatible: true,
            selected_asset: Some(super_stt_shared::registry::SelectedAsset {
                target: String::new(),
                accel: vec!["local".into()],
                cuda_major: None,
                cuda_sm: None,
                cudnn: false,
            }),
            reason: None,
            needs_client_update: false,
        }
    } else {
        // Unlike the install path, an incompatible host is not an error here —
        // "this machine cannot run it" is the single most useful thing a
        // preview can say, and saying it needs the entry, not a 422.
        let sel = compat::select(&host_detect::detect(), &entry);
        Compatibility {
            compatible: sel.reason().is_none(),
            selected_asset: compat::to_selected_asset(&entry, &sel),
            reason: sel.reason().map(ToOwned::to_owned),
            needs_client_update: sel.needs_client_update(),
        }
    };

    let installed = {
        let backends = s.daemon.backends.read().await;
        super::list::installed_version_for_source(&backends, &entry.source)
    };

    let resp = PreviewResponse {
        backend: super::list::map_entry(&entry, compat_field, installed),
        warning: (body.repo_url.is_some() || body.local_path.is_some())
            .then(|| "unverified_source".to_string()),
    };

    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string(&resp).unwrap_or_default(),
    )
        .into_response()
}
