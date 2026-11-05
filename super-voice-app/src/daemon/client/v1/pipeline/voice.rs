// SPDX-License-Identifier: GPL-3.0-only
//! `/tts/pipeline/{stage}/model/{model}/voice` — one synthesis model's voice.
//!
//! Which voice the model speaks in when an utterance names none, and which it
//! could be set to. Stored per `(source, model)` by the daemon, so it survives
//! a model switch rather than following whichever model is loaded.
//!
//! Addressed through the stage, like [`super::device`] and [`super::language`],
//! and split the same way: the preference is one endpoint, the voices on offer
//! another, because picking one rewrites the first and cannot change the
//! second. Only text to speech's stage has a voice, so the routes name it.
//!
//! This is the model's *voice*, not its language: the voice decides who speaks,
//! the language how the text is pronounced. A voice is also a per-utterance
//! field on `POST /tts/speak`, and one named there wins. This is the default it
//! falls back to, and what every caller with no way to choose gets: a keyboard
//! shortcut, the applet, the Speak button.

use serde::Deserialize;

use crate::daemon::client::internal::session::with_settings_token;
use crate::state::{Stage, VoiceChoice, VoiceResolution};
use super_voice_shared::daemon::http_client::HttpResult;
use super_voice_shared::daemon::http_client::transport;

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

fn voice_path(model: &str) -> String {
    let number = Stage::Synthesis.number();
    format!("/tts/pipeline/{number}/model/{}/voice", enc(model))
}

/// The reply of the three verbs on a model's voice.
#[derive(Deserialize)]
struct VoiceEnvelope {
    /// The resolution that results. An absent block reads as "no voice
    /// chosen" rather than failing the read.
    #[serde(default)]
    voice: Option<VoiceResolution>,
}

/// The reply of the voice list.
#[derive(Deserialize)]
struct VoiceListEnvelope {
    #[serde(default)]
    available_voices: Vec<VoiceChoice>,
}

/// Read how one model's voice resolves
/// (HTTP `GET /tts/pipeline/{stage}/model/{model}/voice`).
///
/// Answers whether or not the model is loaded, which is how a card shows its
/// voice before Load.
pub async fn get_model_voice(model: String) -> HttpResult<VoiceResolution> {
    let path = voice_path(&model);
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            let reply: VoiceEnvelope = transport::get_json(socket, &token, &path).await?;
            Ok(reply.voice.unwrap_or_default())
        }
    })
    .await
}

/// Store the voice one model speaks in
/// (HTTP `POST /tts/pipeline/{stage}/model/{model}/voice`).
///
/// A voice the model cannot resolve is refused rather than stored, so offer
/// only what [`list_model_voices`] returns. Returns the resulting resolution,
/// so a card can render its own click without a second read.
pub async fn set_model_voice(model: String, voice: String) -> HttpResult<VoiceResolution> {
    let path = voice_path(&model);
    let body = serde_json::json!({ "voice": voice });
    with_settings_token(move |socket, token| {
        let (path, body) = (path.clone(), body.clone());
        async move {
            let reply: VoiceEnvelope = transport::post_json(socket, &token, &path, &body).await?;
            Ok(reply.voice.unwrap_or_default())
        }
    })
    .await
}

/// Drop the stored voice, returning the model to its `default_voice`
/// (HTTP `DELETE /tts/pipeline/{stage}/model/{model}/voice`).
///
/// The returned resolution says what it fell back to — which for a model that
/// declares no default is nothing at all, the state where speaking is refused
/// until a voice is chosen again.
pub async fn clear_model_voice(model: String) -> HttpResult<VoiceResolution> {
    let path = voice_path(&model);
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            let reply: VoiceEnvelope = transport::delete_json(socket, &token, &path).await?;
            Ok(reply.voice.unwrap_or_default())
        }
    })
    .await
}

/// The voices `model` can be pinned to
/// (HTTP `GET /tts/pipeline/{stage}/model/{model}/voice/list`).
///
/// The model's presets, then the stored cloned voices when it clones. Not the
/// manifest's list: a cloning model declares no presets, so a picker built from
/// the manifest would be empty for exactly the models that refuse to speak
/// without a voice.
pub async fn list_model_voices(model: String) -> HttpResult<Vec<VoiceChoice>> {
    let number = Stage::Synthesis.number();
    let path = format!("/tts/pipeline/{number}/model/{}/voice/list", enc(&model));
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            let reply: VoiceListEnvelope = transport::get_json(socket, &token, &path).await?;
            Ok(reply.available_voices)
        }
    })
    .await
}
