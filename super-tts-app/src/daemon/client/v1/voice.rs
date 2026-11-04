// SPDX-License-Identifier: GPL-3.0-only
//! `/voice` — the cloned-voice library.
//!
//! A voice is a reference recording plus the id (`voice:<uuid>`) that names it
//! on `POST /speak`. These calls own the recording; speaking in one is
//! [`super::speak::speak_in_voice`].
//!
//! The upload is the one call in the app that does not send JSON: the body is
//! the WAV file itself and the metadata rides in the query string, because a
//! recording is megabytes of PCM and base64 in an envelope would inflate it by
//! a third for nothing.
//!
//! These paths carry the daemon's `voices` scope, not `settings`. They are
//! recordings of a person, and the daemon gates them separately for that reason
//! — the app's cached token asks for both, and the consent popup names them in
//! those terms rather than folding them into "every daemon setting".

use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;
use super_tts_shared::models::voices::{
    VoiceDeletedResponse, VoiceInfo, VoiceListResponse, VoiceModelSupport, VoiceResponse,
};

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// The library plus what the loaded model can do with it
/// (HTTP `GET /voice/list`).
///
/// Returns both halves because they arrive together and a UI needs both to
/// render one card: the voices, and whether the loaded model can speak in any
/// of them. `None` for the second means nothing is loaded — the library is
/// still readable, it just cannot be spoken with yet.
pub async fn list_voices() -> HttpResult<(Vec<VoiceInfo>, Option<VoiceModelSupport>)> {
    with_settings_token(|socket, token| async move {
        let resp: VoiceListResponse = transport::get_json(socket, &token, "/voice/list").await?;
        Ok((resp.voices, resp.model))
    })
    .await
}

/// Add a voice from a WAV file (HTTP `POST /voice`).
///
/// `audio` is a complete WAV file. The daemon downmixes, resamples, and stores
/// it — the app sends whatever the recorder or the file produced rather than
/// converting first, so one implementation of that conversion serves every
/// client.
///
/// An empty or whitespace-only transcript is dropped from the query rather than
/// sent: the key's absence is what says "no transcript", where an empty value
/// would be stored as one and shown to a model that clones in context.
pub async fn create_voice(
    label: String,
    transcript: Option<String>,
    audio: Vec<u8>,
) -> HttpResult<VoiceInfo> {
    with_settings_token(move |socket, token| {
        let (label, transcript, audio) = (label.clone(), transcript.clone(), audio.clone());
        async move {
            let transcript = transcript
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .map_or_else(String::new, |t| format!("&transcript={}", enc(t)));
            let path = format!("/voice?label={}{transcript}", enc(&label));
            let resp: VoiceResponse =
                transport::post_bytes(socket, &token, &path, "audio/wav", audio).await?;
            Ok(resp.voice)
        }
    })
    .await
}

/// Rename a voice (HTTP `PATCH /voice/{id}`).
///
/// Only the label is editable. The transcript describes the stored audio and
/// backends cache what they derived from the pair, so changing it would leave
/// every registration already made disagreeing with the library.
pub async fn rename_voice(id: String, label: String) -> HttpResult<VoiceInfo> {
    with_settings_token(move |socket, token| {
        let (id, label) = (id.clone(), label.clone());
        async move {
            let path = format!("/voice/{}", enc(&id));
            let resp: VoiceResponse = transport::patch_json(
                socket,
                &token,
                &path,
                &serde_json::json!({ "label": label }),
            )
            .await?;
            Ok(resp.voice)
        }
    })
    .await
}

/// Delete a voice and its recording (HTTP `DELETE /voice/{id}`).
///
/// The daemon also drops the voice from the loaded model, so speech cannot keep
/// using a recording the user just deleted.
pub async fn delete_voice(id: String) -> HttpResult<String> {
    with_settings_token(move |socket, token| {
        let id = id.clone();
        async move {
            let path = format!("/voice/{}", enc(&id));
            let resp: VoiceDeletedResponse = transport::delete_json(socket, &token, &path).await?;
            Ok(resp.deleted)
        }
    })
    .await
}
