// SPDX-License-Identifier: GPL-3.0-only
//! `/language` — the global transcription language.
//!
//! The per-model override that resolves against this one is
//! [`crate::daemon::client::v1::backends::model_language`].

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

/// Read the global primary language (HTTP `GET /language`).
/// Returns `None` when unset (daemon will auto-detect or use the model default).
pub async fn get_primary_language() -> HttpResult<Option<String>> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/language").await?,
            "get_primary_language",
        )?;
        Ok(resp
            .language
            .and_then(|v| v.as_str().map(ToString::to_string)))
    })
    .await
}

/// Store the global primary language (HTTP `POST /language`).
pub async fn set_primary_language(language: String) -> HttpResult<()> {
    with_settings_token(move |socket, token| {
        let language = language.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                "/language",
                &serde_json::json!({ "language": language }),
            )
            .await?;
            require_unit(resp, "set_primary_language")
        }
    })
    .await
}

/// Clear the global primary language (HTTP `DELETE /language`).
pub async fn clear_primary_language() -> HttpResult<()> {
    with_settings_token(|socket, token| async move {
        let resp = transport::settings_delete(socket, &token, "/language").await?;
        require_unit(resp, "clear_primary_language")
    })
    .await
}
