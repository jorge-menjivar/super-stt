// SPDX-License-Identifier: GPL-3.0-only
//! `/backend/list` — the backends installed on this machine.
//!
//! Mirrors the daemon's `v1/backends/` tree: the catalog and one backend's
//! removal are here, [`options`] and [`secrets`] wrap a backend's configuration.
//! Installing is the registry's job, in [`super::registry`]; pointing a *stage*
//! at one of these backends is [`super::pipeline::stage`]'s.
//!
//! Removal lives here rather than beside install because it is a property of
//! what is already on disk: the registry knows what *could* be installed, and a
//! backend that came from a local directory was never in it at all.

pub(crate) mod options;
pub(crate) mod secrets;

use crate::daemon::backends::BackendInfo;
use crate::daemon::client::internal::response::require_success;
use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::transport;
use super_tts_shared::daemon::http_client::{HttpError, HttpResult};
use super_tts_shared::registry::UninstallResponse;

/// List installed backends with the models, voices, secrets, and options they
/// declare (HTTP `GET /backend/list`). An empty or absent catalog yields an
/// empty `Vec`.
///
/// The whole catalog, regardless of what any stage can run. The narrower read a
/// stage's picker wants — only the backends that stage will accept — is
/// [`super::pipeline::backend::list_stage_backends`].
pub async fn list_backends() -> HttpResult<Vec<BackendInfo>> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/backend/list").await?,
            "list_backends",
        )?;
        match resp.backends {
            Some(value) => serde_json::from_value(value)
                .map_err(|e| HttpError::Other(format!("failed to parse backends: {e}"))),
            None => Ok(Vec::new()),
        }
    })
    .await
}

/// Remove an installed backend and its files (HTTP `DELETE /backend/{source}`).
///
/// `source` is percent-encoded into the path because it is a repo id with
/// slashes in it (`github.com/jorge-menjivar/super-tts-kokoro`); left raw it
/// would read as three more path segments and answer `404` on a backend that is
/// installed.
///
/// The response's `was_active` says whether the synthesis stage was emptied on
/// the way out, which is how a caller knows to stop showing a model that is no
/// longer loaded rather than waiting for a poll to contradict it.
pub async fn uninstall(source: &str) -> HttpResult<UninstallResponse> {
    let encoded = urlencoding::encode(source).into_owned();
    with_settings_token(move |socket, token| {
        let encoded = encoded.clone();
        async move {
            transport::delete_json::<UninstallResponse>(
                socket,
                &token,
                &format!("/backend/{encoded}"),
            )
            .await
        }
    })
    .await
}
