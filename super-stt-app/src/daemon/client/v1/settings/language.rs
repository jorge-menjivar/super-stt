// SPDX-License-Identifier: GPL-3.0-only
//! `/language` + `/backends/{source}/models/{model}/language` — transcription
//! language settings.

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

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

/// Read a specific model's resolved language block
/// (HTTP `GET /backends/{source}/models/{model}/language`).
/// Returns the full resolution `Value` (object with `effective`, `source`,
/// `multilingual`, `supported`, `primary`, `override`), or `Value::Null`
/// when the model is not found.
pub async fn get_model_language(
    source: String,
    model: String,
) -> HttpResult<crate::state::LanguageResolution> {
    with_settings_token(move |socket, token| {
        let (source, model) = (source.clone(), model.clone());
        async move {
            let path = format!("/backends/{}/models/{}/language", enc(&source), enc(&model));
            let resp = require_success(
                transport::settings_get(socket, &token, &path).await?,
                "get_model_language",
            )?;
            // Deserialize the block into a typed resolution here at the boundary;
            // an absent or malformed block yields the empty default rather than
            // being poked field-by-field in the views.
            Ok(resp
                .language
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default())
        }
    })
    .await
}

/// Override a specific model's language
/// (HTTP `POST /backends/{source}/models/{model}/language`).
/// Returns the updated resolution block.
pub async fn set_model_language(
    source: String,
    model: String,
    language: String,
) -> HttpResult<crate::state::LanguageResolution> {
    with_settings_token(move |socket, token| {
        let (source, model, language) = (source.clone(), model.clone(), language.clone());
        async move {
            let path = format!("/backends/{}/models/{}/language", enc(&source), enc(&model));
            let resp = require_success(
                transport::settings_post(
                    socket,
                    &token,
                    &path,
                    &serde_json::json!({ "language": language }),
                )
                .await?,
                "set_model_language",
            )?;
            Ok(resp
                .language
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default())
        }
    })
    .await
}

/// Clear a specific model's language override
/// (HTTP `DELETE /backends/{source}/models/{model}/language`).
/// Returns the updated resolution block.
pub async fn clear_model_language(
    source: String,
    model: String,
) -> HttpResult<crate::state::LanguageResolution> {
    with_settings_token(move |socket, token| {
        let (source, model) = (source.clone(), model.clone());
        async move {
            let path = format!("/backends/{}/models/{}/language", enc(&source), enc(&model));
            let resp = require_success(
                transport::settings_delete(socket, &token, &path).await?,
                "clear_model_language",
            )?;
            Ok(resp
                .language
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default())
        }
    })
    .await
}
