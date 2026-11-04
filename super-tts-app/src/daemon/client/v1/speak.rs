// SPDX-License-Identifier: GPL-3.0-only
//! `/speak` — synthesize text and play it through the daemon.
//!
//! Both calls are ordinary requests, not streams: the daemon answers as soon as
//! the utterance is queued and keeps playing after the connection closes. The
//! Speech page follows what happens next through the `speaking_state` /
//! `speech_progress` topics on `GET /events`, which is also what keeps its
//! badge right when something *else* asked the daemon to speak.

use crate::daemon::client::internal::response::require_unit;
use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client;
use super_tts_shared::daemon::http_client::HttpResult;

/// `POST /speak` — speak `text` with the active model and its default voice.
///
/// Returns the utterance id the daemon assigned, so the caller can tell its own
/// utterance apart from one another client started.
pub async fn speak_command(text: String) -> HttpResult<Option<String>> {
    with_settings_token(|socket, token| {
        let text = text.clone();
        async move {
            let resp = http_client::speak(socket, &token, &text).await?;
            let utterance_id = resp.utterance_id.clone();
            require_unit(resp, "speak_command")?;
            Ok(utterance_id)
        }
    })
    .await
}

/// Select `voice` for the model on `stage`, then speak `text` in it.
///
/// The Voices page's preview: a cloned voice is only judgeable by ear, and the
/// daemon already owns the output device, so hearing one is a normal utterance
/// rather than anything the app plays itself.
///
/// Two calls, because a speak request has no voice field: which voice the
/// machine speaks in is a setting, so auditioning one *is* selecting it. That
/// is the honest shape — the preview a user listens to is the voice they will
/// get from every other client afterwards, rather than a sound only this page
/// can produce. The selection is left in place, which is what the button means.
///
/// The utterance id is discarded — a preview is not something the page later
/// correlates or cancels, and the speaking badge is driven by events either
/// way.
pub async fn speak_in_voice(
    stage: u32,
    model: String,
    voice: String,
    text: String,
) -> HttpResult<()> {
    super::pipeline::voice::set_model_voice(stage, model, voice).await?;
    speak_command(text).await.map(|_| ())
}

/// `POST /speak/stop` — stop the current utterance and drop queued audio.
pub async fn stop_speaking_command() -> HttpResult<()> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::speak_stop(socket, &token).await?;
        require_unit(resp, "stop_speaking_command")
    })
    .await
}
