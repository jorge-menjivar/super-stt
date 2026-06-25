// SPDX-License-Identifier: GPL-3.0-only
//! `/language` + `/active_model/language` — transcription language settings.

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;

/// Read the global primary language (HTTP `GET /language`).
/// Returns `None` when unset (daemon will auto-detect or use the model default).
pub async fn get_primary_language() -> Result<Option<String>, String> {
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
pub async fn set_primary_language(language: String) -> Result<(), String> {
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
pub async fn clear_primary_language() -> Result<(), String> {
    with_settings_token(|socket, token| async move {
        let resp = transport::settings_delete(socket, &token, "/language").await?;
        require_unit(resp, "clear_primary_language")
    })
    .await
}

/// Read the active model's resolved language block (HTTP `GET /active_model/language`).
/// Returns the full resolution `Value` (object with `effective`, `source`, etc.),
/// or `Value::Null` when no model is loaded.
pub async fn get_active_model_language() -> Result<serde_json::Value, String> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/active_model/language").await?,
            "get_active_model_language",
        )?;
        Ok(resp.language.unwrap_or(serde_json::Value::Null))
    })
    .await
}

/// Override the active model's language (HTTP `POST /active_model/language`).
/// Returns the updated resolution block.
pub async fn set_active_model_language(language: String) -> Result<serde_json::Value, String> {
    with_settings_token(move |socket, token| {
        let language = language.clone();
        async move {
            let resp = require_success(
                transport::settings_post(
                    socket,
                    &token,
                    "/active_model/language",
                    &serde_json::json!({ "language": language }),
                )
                .await?,
                "set_active_model_language",
            )?;
            Ok(resp.language.unwrap_or(serde_json::Value::Null))
        }
    })
    .await
}

/// Clear the active model's language override (HTTP `DELETE /active_model/language`).
/// Returns the updated resolution block.
pub async fn clear_active_model_language() -> Result<serde_json::Value, String> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_delete(socket, &token, "/active_model/language").await?,
            "clear_active_model_language",
        )?;
        Ok(resp.language.unwrap_or(serde_json::Value::Null))
    })
    .await
}
