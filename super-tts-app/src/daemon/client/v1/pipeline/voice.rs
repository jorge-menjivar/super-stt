// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model/{model}/voice` — one model's voice.
//!
//! Which voice the model speaks in when an utterance names none, and which it
//! could be set to. Stored per `(source, model)` by the daemon, so it survives
//! a model switch rather than following whichever model is loaded.
//!
//! Addressed through the stage, like [`super::device`] and [`super::language`],
//! and split the same way: the preference is one endpoint, the voices on offer
//! another, because picking one rewrites the first and cannot change the
//! second.
//!
//! This is the model's *voice*, not its language: the voice decides who speaks,
//! the language how the text is pronounced. A voice is still a per-utterance
//! field on `POST /speak` as well, and one named there wins — that is what the
//! Voices page's preview uses. This is the default it falls back to, and what
//! every caller with no UI for choosing gets: a keyboard shortcut, the applet,
//! the Speak button.

use crate::daemon::client::internal::response::require_success;
use crate::daemon::client::internal::session::with_settings_token;
use crate::state::{VoiceChoice, VoiceResolution};
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

fn voice_path(stage: u32, model: &str) -> String {
    format!("/pipeline/{stage}/model/{}/voice", enc(model))
}

/// Deserialize the resolution block at the boundary, so the views never poke
/// at a raw JSON value. An absent or malformed block yields the default, which
/// renders as "no voice chosen" rather than as a crash.
fn block(value: Option<serde_json::Value>) -> VoiceResolution {
    value
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Read how one model's voice resolves
/// (HTTP `GET /pipeline/{stage}/model/{model}/voice`).
///
/// Answers whether or not the model is loaded, which is how a card shows its
/// voice before Load.
pub async fn get_model_voice(stage: u32, model: String) -> HttpResult<VoiceResolution> {
    with_settings_token(move |socket, token| {
        let model = model.clone();
        async move {
            let path = voice_path(stage, &model);
            let resp = require_success(
                transport::settings_get(socket, &token, &path).await?,
                "get_model_voice",
            )?;
            Ok(block(resp.voice))
        }
    })
    .await
}

/// Store the voice one model speaks in
/// (HTTP `POST /pipeline/{stage}/model/{model}/voice`).
///
/// A voice the model cannot resolve is refused rather than stored, so offer
/// only what [`list_model_voices`] returns. Returns the resulting resolution,
/// so a card can render its own click without a second read.
pub async fn set_model_voice(
    stage: u32,
    model: String,
    voice: String,
) -> HttpResult<VoiceResolution> {
    with_settings_token(move |socket, token| {
        let (model, voice) = (model.clone(), voice.clone());
        async move {
            let path = voice_path(stage, &model);
            let resp = require_success(
                transport::settings_post(
                    socket,
                    &token,
                    &path,
                    &serde_json::json!({ "voice": voice }),
                )
                .await?,
                "set_model_voice",
            )?;
            Ok(block(resp.voice))
        }
    })
    .await
}

/// Drop the stored voice, returning the model to its `default_voice`
/// (HTTP `DELETE /pipeline/{stage}/model/{model}/voice`).
///
/// The returned resolution says what it fell back to — which for a model that
/// declares no default is nothing at all, the state where speaking is refused
/// until a voice is chosen again.
pub async fn clear_model_voice(stage: u32, model: String) -> HttpResult<VoiceResolution> {
    with_settings_token(move |socket, token| {
        let model = model.clone();
        async move {
            let path = voice_path(stage, &model);
            let resp = require_success(
                transport::settings_delete(socket, &token, &path).await?,
                "clear_model_voice",
            )?;
            Ok(block(resp.voice))
        }
    })
    .await
}

/// The voices `model` can be pinned to
/// (HTTP `GET /pipeline/{stage}/model/{model}/voice/list`).
///
/// The model's presets, then the stored cloned voices when it clones. Not the
/// manifest's list: a cloning model declares no presets, so a picker built from
/// the manifest would be empty for exactly the models that refuse to speak
/// without a voice.
pub async fn list_model_voices(stage: u32, model: String) -> HttpResult<Vec<VoiceChoice>> {
    with_settings_token(move |socket, token| {
        let model = model.clone();
        async move {
            let path = format!("/pipeline/{stage}/model/{}/voice/list", enc(&model));
            let resp = require_success(
                transport::settings_get(socket, &token, &path).await?,
                "list_model_voices",
            )?;
            Ok(resp
                .available_voices
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect())
        }
    })
    .await
}
