// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model/{model}/language` — one model's language.
//!
//! A per-model override, and the resolution that answers which language is
//! actually in effect: the override, the global `/settings/language` setting,
//! or the model's own default. The global setting these resolve against is
//! [`crate::daemon::client::v1::settings::language`].
//!
//! Addressed through the stage, like [`super::device`]: the stage resolves a
//! bare model name against the backend filling it, so the caller does not have
//! to carry a `source` it already told the daemon about.
//!
//! And split like it, too: the override is one endpoint, the languages on offer
//! another. One answers what is set, the other what can be set.

use crate::daemon::client::internal::response::require_success;
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// Read a specific model's resolved language block
/// (HTTP `GET /pipeline/{stage}/model/{model}/language`).
/// Returns the full resolution `Value` (object with `effective`, `source`,
/// `multilingual`, `supported`, `primary`, `override`), or `Value::Null`
/// when the model is not found.
pub async fn get_model_language(
    stage: u32,
    model: String,
) -> HttpResult<crate::state::LanguageResolution> {
    with_settings_token(move |socket, token| {
        let model = model.clone();
        async move {
            let path = format!("/pipeline/{stage}/model/{}/language", enc(&model));
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
/// (HTTP `POST /pipeline/{stage}/model/{model}/language`).
/// Returns the updated resolution block.
pub async fn set_model_language(
    stage: u32,
    model: String,
    language: String,
) -> HttpResult<crate::state::LanguageResolution> {
    with_settings_token(move |socket, token| {
        let (model, language) = (model.clone(), language.clone());
        async move {
            let path = format!("/pipeline/{stage}/model/{}/language", enc(&model));
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
/// (HTTP `DELETE /pipeline/{stage}/model/{model}/language`).
/// Returns the updated resolution block.
pub async fn clear_model_language(
    stage: u32,
    model: String,
) -> HttpResult<crate::state::LanguageResolution> {
    with_settings_token(move |socket, token| {
        let model = model.clone();
        async move {
            let path = format!("/pipeline/{stage}/model/{}/language", enc(&model));
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

/// The languages `model` can be pinned to
/// (HTTP `GET /pipeline/{stage}/model/{model}/language/list`).
///
/// What the setter accepts — the model's own tags plus the reserved `auto` —
/// not a general BCP-47 list. Empty for a monolingual model, which is what
/// tells a picker there is nothing to choose.
pub async fn list_model_languages(stage: u32, model: String) -> HttpResult<Vec<String>> {
    with_settings_token(move |socket, token| {
        let model = model.clone();
        async move {
            let path = format!("/pipeline/{stage}/model/{}/language/list", enc(&model));
            let resp = require_success(
                transport::settings_get(socket, &token, &path).await?,
                "list_model_languages",
            )?;
            Ok(resp.available_languages.unwrap_or_default())
        }
    })
    .await
}
