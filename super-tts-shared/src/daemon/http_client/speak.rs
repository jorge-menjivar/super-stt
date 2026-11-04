// SPDX-License-Identifier: GPL-3.0-only
//! Client half of `POST /v1/speak` and `POST /v1/speak/stop`: Super TTS's own
//! endpoints, on the transport it shares with Super STT.
//!
//! Speaking is fire-and-forget by design: the daemon answers `202` as soon as
//! the utterance is queued and keeps playing after the connection closes, so
//! there is no streaming variant here. A client that wants to follow the
//! utterance subscribes to the `speaking_state` / `speech_progress` topics on
//! `GET /events` instead — those outlive any one request, which is what a
//! status widget actually needs.
//!
//! The request is `text` and nothing else. A voice, a language and a rate are
//! settings the user owns, reached through the settings endpoints; an utterance
//! reads them, it does not choose them.

use std::path::PathBuf;

use super_engine_client::http_client::HttpResult;
use super_engine_client::http_client::transport;

use crate::models::protocol::DaemonResponse;

/// `POST /speak` — synthesize `text` with the active model and play it.
///
/// Returns once the utterance is *queued*, not once it finishes: the response
/// carries `utterance_id`, which is what correlates the playback events with
/// this call.
///
/// `text` is the whole request. How it is spoken — the voice, the language, the
/// rate — is configuration, set through the settings endpoints and read by the
/// daemon per utterance, not something a speaking client chooses. See
/// `docs/protocol/endpoints/v1/speak.md`.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable or the
/// response can't be parsed.
pub async fn speak(socket_path: PathBuf, token: &str, text: &str) -> HttpResult<DaemonResponse> {
    transport::post_json(
        socket_path,
        token,
        "/speak",
        &serde_json::json!({ "text": text }),
    )
    .await
}

/// `POST /speak/stop` — stop the current utterance and drop queued audio.
///
/// Succeeds whether or not anything was speaking, so a client cancelling on a
/// keypress doesn't have to win a race to get a clean result.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable or the
/// response can't be parsed.
pub async fn speak_stop(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    transport::post_json(socket_path, token, "/speak/stop", &serde_json::json!({})).await
}
