// SPDX-License-Identifier: GPL-3.0-only
//! `/backends` — the backends installed on this machine.
//!
//! Mirrors the daemon's `v1/backends/` tree: the catalog and one backend's
//! removal are here, [`options`] and [`secrets`] wrap a backend's configuration,
//! [`model_language`] its per-model language override.
//!
//! Installing is the registry's job, in [`super::registry`]; filling a *stage*
//! with one of these backends is [`super::pipeline::stage`]'s.

pub(crate) mod model_language;
pub(crate) mod options;
pub(crate) mod secrets;

use crate::daemon::backends::BackendInfo;
use crate::daemon::client::internal::response::require_success;
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;
use super_stt_shared::daemon::http_client::{HttpError, HttpResult};
use super_stt_shared::registry::UninstallResponse;

/// List installed backends with the models, secrets, and options they
/// declare (HTTP `GET /backends`). An empty or absent catalog yields an
/// empty `Vec`.
pub async fn list_backends() -> HttpResult<Vec<BackendInfo>> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/backends").await?,
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

/// `DELETE /backends/{source}` — uninstall a backend.
pub async fn uninstall(source: &str) -> HttpResult<UninstallResponse> {
    let encoded = urlencoding::encode(source).into_owned();
    with_settings_token(move |socket, token| {
        let encoded = encoded.clone();
        async move {
            transport::delete_json::<UninstallResponse>(
                socket,
                &token,
                &format!("/backends/{encoded}"),
            )
            .await
        }
    })
    .await
}
