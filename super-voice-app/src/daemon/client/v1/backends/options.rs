// SPDX-License-Identifier: GPL-3.0-only
//! `/backend/{source}/option/{name}` — a backend's non-sensitive settings.
//!
//! Values the backend declares as `[[options]]` — a base URL, a timeout. Unlike
//! [`super::secrets`], these read back: they are stored as plaintext and the
//! daemon returns them.

use crate::daemon::client::internal::response::require_unit;
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

/// Set a backend option (HTTP `POST /backend/{source}/option/{name}`).
pub async fn set_backend_option(source: String, name: String, value: String) -> HttpResult<()> {
    with_settings_token(move |socket, token| {
        let (source, name, value) = (source.clone(), name.clone(), value.clone());
        async move {
            let path = format!(
                "/backend/{}/option/{}",
                urlencoding::encode(&source),
                urlencoding::encode(&name)
            );
            let resp = transport::settings_post(
                socket,
                &token,
                &path,
                &serde_json::json!({ "value": value }),
            )
            .await?;
            require_unit(resp, "set_backend_option")
        }
    })
    .await
}

/// Clear a backend option (HTTP `DELETE /backend/{source}/option/{name}`).
pub async fn clear_backend_option(source: String, name: String) -> HttpResult<()> {
    with_settings_token(move |socket, token| {
        let (source, name) = (source.clone(), name.clone());
        async move {
            let path = format!(
                "/backend/{}/option/{}",
                urlencoding::encode(&source),
                urlencoding::encode(&name)
            );
            let resp = transport::settings_delete(socket, &token, &path).await?;
            require_unit(resp, "clear_backend_option")
        }
    })
    .await
}
