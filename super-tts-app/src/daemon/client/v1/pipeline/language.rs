// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model/{model}/language` — one model's language.
//!
//! A per-model override, and the resolution that answers which language is
//! actually in effect: the override, the global `/settings/language` setting,
//! or the model's own primary language. The global setting these resolve
//! against is [`crate::daemon::client::v1::settings::language`].
//!
//! This is the model's *language*, not its voice. The language decides how text
//! is pronounced; the voice decides who says it, and a voice is a per-utterance
//! field on `POST /speak` rather than a stored preference.
//!
//! Addressed through the stage, like [`super::device`], and for the same
//! reason: the daemon keys the override by `(source, model)`, and the stage is
//! what resolves a bare model name against the backend filling it. Guessing the
//! source by scanning for whoever serves the name would write one backend's
//! preference onto another's model. It moved here from
//! `/backends/{source}/models/{model}/language`, which could reach any
//! installed model; requiring the backend to be filling a stage costs nothing
//! real, since a language control is only ever shown on a stage's card.
//!
//! And split like [`super::device`], too: the override is one endpoint, the
//! languages on offer another. One answers what is set, the other what can be
//! set, and only one of them changes when the user picks a language.

use crate::daemon::client::internal::response::require_success;
use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

fn language_path(stage: u32, model: &str) -> String {
    format!("/pipeline/{stage}/model/{}/language", enc(model))
}

/// Read how one model's language resolves
/// (HTTP `GET /pipeline/{stage}/model/{model}/language`).
///
/// The whole resolution rather than the stored value alone: a card showing
/// "Automatic" still has to say what automatic resolved to, and three settings
/// can decide it. `override` is `null` when none is set, which is what
/// "follows the global setting" looks like.
///
/// Answers whether or not the model is loaded, which is how a card shows its
/// language before Load.
pub async fn get_model_language(
    stage: u32,
    model: String,
) -> HttpResult<crate::state::LanguageResolution> {
    with_settings_token(move |socket, token| {
        let model = model.clone();
        async move {
            let path = language_path(stage, &model);
            let resp = require_success(
                transport::settings_get(socket, &token, &path).await?,
                "get_model_language",
            )?;
            // Deserialize the block into a typed resolution here at the
            // boundary; an absent or malformed block yields the empty default
            // rather than being poked field-by-field in the views.
            Ok(resp
                .language
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default())
        }
    })
    .await
}

/// Pin one model to a language
/// (HTTP `POST /pipeline/{stage}/model/{model}/language`).
///
/// Stored against the backend and model together, so it survives model switches
/// rather than following whichever model happens to be loaded. Returns the
/// resolution that results, so a card can render its own click without a second
/// read.
///
/// A tag the model does not serve is refused rather than ignored — the
/// alternative is an utterance that comes out mispronounced or not at all — so
/// offer only what [`list_model_languages`] returns.
pub async fn set_model_language(
    stage: u32,
    model: String,
    language: String,
) -> HttpResult<crate::state::LanguageResolution> {
    with_settings_token(move |socket, token| {
        let (model, language) = (model.clone(), language.clone());
        async move {
            let path = language_path(stage, &model);
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

/// Remove the pin, returning the model to Automatic
/// (HTTP `DELETE /pipeline/{stage}/model/{model}/language`).
///
/// Distinct from setting `auto`, which is itself a stated preference: this
/// falls back to the global setting, and to the model's own primary language
/// when that is unset too. The returned resolution says which of the two
/// happened.
pub async fn clear_model_language(
    stage: u32,
    model: String,
) -> HttpResult<crate::state::LanguageResolution> {
    with_settings_token(move |socket, token| {
        let model = model.clone();
        async move {
            let path = language_path(stage, &model);
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
/// What [`set_model_language`] will accept — the model's own tags plus the
/// reserved `auto` — not a general BCP-47 list. Which of `en` and `en-US` a
/// model answers to is a rule only the daemon's resolver knows, and a picker
/// that guessed would offer tags that are refused on click.
///
/// Empty for a monolingual model, whatever its manifest lists: that is what
/// tells a picker there is nothing to choose, and it hides the control rather
/// than special-casing a status.
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
