// SPDX-License-Identifier: GPL-3.0-only
//! `/backend/{source}/option/{name}` — a backend's non-sensitive settings.
//!
//! Values the backend declares as `[[options]]` — a base URL, a speaking-style
//! preset, a timeout. Unlike [`super::secrets`], these read back: they are
//! stored as plaintext and the daemon returns them, so a settings form can show
//! what is in effect. The current values arrive with the catalog on
//! `GET /backend/list`, which is why only the two writes are wrapped here.
//!
//! Writing one reloads whatever model the pipeline is running from that
//! backend, so the new value takes effect without the user unloading by hand.

use crate::daemon::client::internal::response::require_unit;
use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;

/// Store an override for one option
/// (HTTP `POST /backend/{source}/option/{name}`).
///
/// Both segments are percent-encoded: `source` is a repo id full of slashes,
/// and an option name is only constrained by the manifest that declared it.
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

/// Drop the override, reverting the option to its manifest default
/// (HTTP `DELETE /backend/{source}/option/{name}`).
///
/// This rather than posting a copy of the default: storing today's default
/// would pin the user to it, and a later manifest that moved the default would
/// leave them on the old value with nothing on screen to say why.
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
