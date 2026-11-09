// SPDX-License-Identifier: GPL-3.0-only
//! `/tts/speak` — synthesize text and play it through the daemon.
//!
//! Both calls are ordinary requests, not streams: the daemon answers as soon as
//! the utterance is queued and keeps playing after the connection closes. The
//! Speech tab follows what happens next through the `speaking_state` topic on
//! `GET /events`, which is also what keeps its badge right when something
//! *else* asked the daemon to speak.

use serde::Deserialize;

use crate::daemon::client::internal::session::with_settings_token;
use super_voice_shared::daemon::http_client::{HttpError, HttpResult, transport};

/// The reply to a speak request.
#[derive(Deserialize)]
struct SpeakReply {
    #[serde(default)]
    status: String,
    #[serde(default)]
    message: Option<String>,
    /// The id the daemon gave the utterance.
    #[serde(default)]
    utterance_id: Option<String>,
}

impl SpeakReply {
    fn require_success(self, context: &str) -> HttpResult<Self> {
        if self.status == "success" {
            Ok(self)
        } else {
            Err(HttpError::Other(
                self.message
                    .clone()
                    .unwrap_or_else(|| format!("{context} failed")),
            ))
        }
    }
}

/// `POST /tts/speak` — speak `text` with the synthesis model and its voice.
///
/// Returns the utterance id the daemon assigned, so the caller can tell its own
/// utterance apart from one another client started.
pub async fn speak_command(text: String) -> HttpResult<Option<String>> {
    let body = serde_json::json!({ "text": text });
    with_settings_token(move |socket, token| {
        let body = body.clone();
        async move {
            let reply: SpeakReply =
                transport::post_json(socket, &token, "/tts/speak", &body).await?;
            Ok(reply.require_success("speak_command")?.utterance_id)
        }
    })
    .await
}

/// Select `voice` for the synthesis `model`, then speak `text` in it.
///
/// The Voices tab's preview: a cloned voice is only judgeable by ear, and the
/// daemon already owns the output device, so hearing one is a normal utterance
/// rather than anything the app plays itself.
///
/// Two calls, because a speak request has no voice field: which voice the
/// machine speaks in is a setting, so auditioning one *is* selecting it. That
/// is the honest shape — the preview a user listens to is the voice they will
/// get from every other client afterwards, rather than a sound only this tab
/// can produce. The selection is left in place, which is what the button means.
///
/// The utterance id is discarded — a preview is not something the tab later
/// correlates or cancels, and the speaking badge is driven by events either
/// way.
pub async fn speak_in_voice(model: String, voice: String, text: String) -> HttpResult<()> {
    super::pipeline::voice::set_model_voice(model, voice).await?;
    speak_command(text).await.map(|_| ())
}

/// `POST /tts/speak/stop` — stop the current utterance and drop queued audio.
pub async fn stop_speaking_command() -> HttpResult<()> {
    with_settings_token(|socket, token| async move {
        let reply: SpeakReply =
            transport::post_json(socket, &token, "/tts/speak/stop", &serde_json::json!({})).await?;
        reply.require_success("stop_speaking_command").map(|_| ())
    })
    .await
}
