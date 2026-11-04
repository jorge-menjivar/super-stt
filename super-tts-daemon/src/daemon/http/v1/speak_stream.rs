// SPDX-License-Identifier: GPL-3.0-only
//! `GET /v1/speak/stream` — speak text as it is produced.
//!
//! The LLM case: a client sends deltas as tokens arrive, and sentences are
//! synthesized as they complete, so speech starts long before generation
//! finishes. [`crate::text`] does the hard part; this module is transport.
//!
//! A WebSocket rather than a series of POSTs specifically for the back-channel.
//! When the user interrupts a half-generated reply the daemon has to cancel
//! in-flight synthesis, drop the queue, flush the ring **and** tell the client
//! to stop generating. A one-way append API makes that last part awkward.
//!
//! Contract: `docs/protocol/endpoints/v1/speak/stream.md`, which is where the
//! frame protocol is written down — a WebSocket's messages are not describable
//! in `OpenAPI`, so the published operation documents only the upgrade.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use log::{debug, warn};
use serde::{Deserialize, Serialize};

use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use crate::daemon::speech::{SpeakError, SpeakSession, SpeechEvent};
use crate::daemon::types::SuperTTSDaemon;

/// Abort a session that has sent no frame for this long.
///
/// An idle stream holds the utterance slot and the output device, so a client
/// that goes away without closing (a half-open TCP connection, a crashed
/// process) must not keep them forever.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Maximum concurrent streaming sessions.
///
/// Only one can be *speaking* — the engine is newest-wins — but each holds a
/// task and a buffer, so an authorized client should not be able to open them
/// without limit.
const MAX_SESSIONS: usize = 4;

/// Permits for [`MAX_SESSIONS`]; `try_acquire` rejects rather than queues.
static SESSIONS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_SESSIONS);

/// A frame from the client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    /// First frame: opens the utterance. Carries nothing — how it is spoken is
    /// the user's configuration, which the daemon reads per utterance.
    Start,
    /// More text. Repeatable.
    Text { delta: String },
    /// No more text is coming; play out what remains.
    End,
    /// Stop now and drop queued audio.
    Cancel,
}

/// Fields the `start` frame used to take, and no longer does.
///
/// Same rule as `POST /speak`: a voice, a language, a rate and delivery
/// guidance are the user's settings, not something a speaking client chooses.
const REMOVED_START_FIELDS: &[&str] = &["voice", "language", "speed", "instructions"];

/// A frame to the client.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerFrame {
    /// The utterance this session is producing. Sent once, after `start`.
    Utterance { id: String },
    /// The backend aligned a span of audio to a span of the text.
    Mark {
        #[serde(flatten)]
        mark: super_tts_shared::audio::frames::Mark,
    },
    /// How far playback has got, in milliseconds.
    Progress { spoken_ms: u64, queued_ms: u64 },
    /// Every sample is queued and the stream closes after this — not that the
    /// audio has been heard. The socket is a text channel, and holding one of
    /// four session slots through a playout the client cannot influence buys
    /// nothing; a client that needs the true end follows `speaking_state` on
    /// `GET /events`, which outlives the socket.
    Done { id: String, chunks: usize },
    /// Fatal; the stream closes after it.
    Error { message: String },
}

#[utoipa::path(
    get,
    path = "/speak/stream",
    tag = "speak",
    summary = "Speak text as it is produced (WebSocket)",
    description = "\
An HTTP/1.1 upgrade to a WebSocket. The client sends text deltas as they arrive; \
the daemon synthesizes each sentence as it completes, so speech starts long before \
generation finishes. This is the LLM case — with `POST /speak` you must hold the \
whole reply before the first word is heard.

The first frame must be `start`. It carries nothing — how an utterance is spoken is \
the user's configuration, read per utterance, the same as on `POST /speak` — and a \
`start` still naming `voice`, `language`, `speed` or `instructions` is refused by name \
rather than having it dropped in silence. Then `text` frames carry deltas, `end` plays \
out what remains, and `cancel` stops immediately and drops the queue. The daemon answers with `utterance`, \
`mark`, `progress`, and a terminal `done` or `error`.

Deltas are appended to a streaming normalizer, so a delta may split a markup \
construct in half without the markers being spoken.

Being a WebSocket, the frame protocol is not describable in OpenAPI — the message \
shapes are in `docs/protocol/endpoints/v1/speak/stream.md`. Four concurrent sessions \
are allowed; beyond that the upgrade is refused rather than queued, and a session \
that sends no frame for two minutes is treated as dead and torn down.",
    // `speak` is what the router enforces; `settings` is required only for a
    // request that names an override field, which is a body-dependent check the
    // handler makes. Declaring it here would tell every client it needs a scope
    // it usually does not — see the description and the 403 below.
    security(("session_token" = ["speak"])),
    responses(
        (status = 101, description = "Upgraded; the streaming session is open."),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `speak` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "Too many concurrent streaming sessions (`speak_sessions_busy`).", body = ErrorEnvelope),
    ),
)]
/// `GET /v1/speak/stream` — upgrade and stream text in, audio out.
pub(crate) async fn speak_stream_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    // Claim the permit before upgrading, so an over-cap client gets a clean
    // `503` rather than a socket that is immediately closed.
    //
    // The refusal carries the house JSON envelope rather than a bare string.
    // It used to send `speak_sessions_busy` as `text/plain`, which is the one
    // shape a client here cannot read: this is the only response on the
    // endpoint that is not a protocol upgrade, so a caller has no other reason
    // to have a body parser attached, and `transport.md` promises `error_code`
    // on every error. It also disagreed with what this operation publishes.
    let Ok(permit) = SESSIONS.try_acquire() else {
        warn!("speak stream rejected: {MAX_SESSIONS} sessions already active");
        return crate::daemon::http::v1::backends::json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "speak_sessions_busy",
        );
    };
    let daemon = Arc::clone(&state.daemon);
    ws.on_upgrade(move |socket| run(socket, daemon, permit))
}

/// Drive one streaming session to completion.
async fn run(
    socket: WebSocket,
    daemon: Arc<SuperTTSDaemon>,
    // Held for the session's lifetime; dropping it frees a permit.
    _permit: tokio::sync::SemaphorePermit<'static>,
) {
    let (mut sink, mut stream) = socket.split();

    // The first frame must be `start`: everything else needs the options it
    // carries, and guessing them would mean a wrong voice rather than an error.
    let session = match wait_for_start(&mut stream, &daemon).await {
        Ok(session) => session,
        Err(message) => {
            let _ = send(&mut sink, &ServerFrame::Error { message }).await;
            let _ = sink.close().await;
            return;
        }
    };
    let mut session = session;

    if send(
        &mut sink,
        &ServerFrame::Utterance {
            id: session.id().to_string(),
        },
    )
    .await
    .is_err()
    {
        return;
    }

    let outcome = pump(&mut stream, &mut sink, &mut session).await;
    match outcome {
        Outcome::Done => {
            let id = session.id().to_string();
            match session.end().await {
                // Terminal frames: the socket closes next either way, so a
                // failed send is nothing left to act on.
                Ok(utterance) => {
                    let _ = send(
                        &mut sink,
                        &ServerFrame::Done {
                            id,
                            chunks: utterance.chunks,
                        },
                    )
                    .await;
                }
                Err(e) => {
                    let _ = send(
                        &mut sink,
                        &ServerFrame::Error {
                            message: e.to_string(),
                        },
                    )
                    .await;
                }
            }
        }
        Outcome::Cancelled => {
            session.cancel().await;
            debug!("speak stream cancelled by client");
        }
        Outcome::Failed(message) => {
            session.cancel().await;
            let _ = send(&mut sink, &ServerFrame::Error { message }).await;
        }
    }
    let _ = sink.close().await;
}

/// How a session's frame loop ended.
enum Outcome {
    /// The client sent `end`, or closed cleanly.
    Done,
    /// The client sent `cancel`, or went away mid-utterance.
    Cancelled,
    /// Something failed; the message is for the client.
    Failed(String),
}

/// Read frames until the session ends.
async fn pump(
    stream: &mut futures::stream::SplitStream<WebSocket>,
    sink: &mut futures::stream::SplitSink<WebSocket, Message>,
    session: &mut SpeakSession,
) -> Outcome {
    loop {
        let next = tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await;
        let Ok(next) = next else {
            return Outcome::Failed("idle_timeout".into());
        };
        let Some(Ok(message)) = next else {
            // A disconnect mid-utterance is a cancel: the listener is gone, and
            // finishing would speak to an empty room while holding the device.
            return Outcome::Cancelled;
        };

        let text = match message {
            Message::Text(t) => t.to_string(),
            // Binary carries nothing in this protocol; ping/pong are handled by
            // axum, and a Close ends the session cleanly.
            Message::Close(_) => return Outcome::Done,
            _ => continue,
        };

        let frame: ClientFrame = match serde_json::from_str(&text) {
            Ok(f) => f,
            Err(e) => return Outcome::Failed(format!("invalid_frame: {e}")),
        };

        match frame {
            ClientFrame::Start => {
                return Outcome::Failed("start_already_sent".into());
            }
            ClientFrame::Text { delta } => {
                if let Err(e) = session.push(&delta).await {
                    return Outcome::Failed(e.to_string());
                }
            }
            ClientFrame::End => return Outcome::Done,
            ClientFrame::Cancel => return Outcome::Cancelled,
        }

        // A later utterance can supersede this one at any time; the client
        // should learn that rather than keep sending into a dead session.
        if session.is_cancelled() {
            return Outcome::Failed("superseded".into());
        }

        // Drain whatever the backend reported while that delta was synthesized.
        while let Some(SpeechEvent::Mark(mark)) = session.try_next_event() {
            if send(sink, &ServerFrame::Mark { mark }).await.is_err() {
                return Outcome::Cancelled;
            }
        }
        let (spoken_ms, queued_ms) = session.progress();
        if send(
            sink,
            &ServerFrame::Progress {
                spoken_ms,
                queued_ms,
            },
        )
        .await
        .is_err()
        {
            return Outcome::Cancelled;
        }
    }
}

/// Read the opening `start` frame and begin a session.
async fn wait_for_start(
    stream: &mut futures::stream::SplitStream<WebSocket>,
    daemon: &SuperTTSDaemon,
) -> Result<SpeakSession, String> {
    let next = tokio::time::timeout(IDLE_TIMEOUT, stream.next())
        .await
        .map_err(|_| "idle_timeout".to_string())?;
    let Some(Ok(Message::Text(text))) = next else {
        return Err("expected_start_frame".into());
    };
    // Parsed as raw JSON first so a frame still carrying the removed fields can
    // be named in the refusal. Serde would simply ignore them, and a client
    // whose chosen voice was dropped in silence has no way to find out.
    let raw: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("invalid_frame: {e}"))?;
    let named: Vec<&str> = REMOVED_START_FIELDS
        .iter()
        .copied()
        .filter(|field| raw.get(*field).is_some_and(|v| !v.is_null()))
        .collect();
    if !named.is_empty() {
        return Err(format!(
            "invalid_frame: {} no longer accepted on start; how an utterance is spoken is \
             configuration — set a voice with POST /pipeline/{{stage}}/model/{{model}}/voice \
             and a language with POST /settings/language",
            named.join(", ")
        ));
    }
    let frame: ClientFrame =
        serde_json::from_value(raw).map_err(|e| format!("invalid_frame: {e}"))?;
    if !matches!(frame, ClientFrame::Start) {
        return Err("expected_start_frame".into());
    }

    // Through the daemon rather than straight to the engine: what the frame
    // does not name comes from the user's configuration, the same as on
    // `POST /speak`. Calling `SpeechEngine::begin` here instead is what left a
    // clone-only model refusing every stream for want of a voice the user had
    // already chosen.
    daemon.begin_speech().await.map_err(|e| match e {
        SpeakError::NotLoaded => "model_not_loaded".to_string(),
        other => other.to_string(),
    })
}

/// Send one frame, reporting whether the socket is still usable.
async fn send(
    sink: &mut futures::stream::SplitSink<WebSocket, Message>,
    frame: &ServerFrame,
) -> Result<(), ()> {
    let Ok(json) = serde_json::to_string(frame) else {
        return Err(());
    };
    sink.send(Message::Text(json.into())).await.map_err(|_| ())
}

pub(crate) fn routes() -> utoipa_axum::router::OpenApiRouter<AppState> {
    utoipa_axum::router::OpenApiRouter::new().routes(utoipa_axum::routes!(speak_stream_handler))
}

#[cfg(test)]
mod tests {
    use super::{ClientFrame, ServerFrame};
    use super_tts_shared::audio::frames::Mark;

    /// The wire shape is the contract with every client, and a field rename in
    /// Rust would silently change it. These pin the exact JSON.
    #[test]
    fn client_frames_parse_from_their_documented_shape() {
        // `start` carries nothing: how the utterance is spoken is configuration.
        assert!(matches!(
            serde_json::from_str::<ClientFrame>(r#"{"type":"start"}"#),
            Ok(ClientFrame::Start)
        ));
        // A frame still naming a removed field parses — serde ignores unknown
        // keys — which is exactly why `wait_for_start` checks the raw JSON
        // before this, rather than letting the voice be dropped in silence.
        assert!(matches!(
            serde_json::from_str::<ClientFrame>(r#"{"type":"start","voice":"af_bella"}"#),
            Ok(ClientFrame::Start)
        ));

        let text: ClientFrame =
            serde_json::from_str(r#"{"type":"text","delta":"Hello"}"#).expect("text parses");
        assert!(matches!(text, ClientFrame::Text { delta } if delta == "Hello"));

        assert!(matches!(
            serde_json::from_str::<ClientFrame>(r#"{"type":"end"}"#),
            Ok(ClientFrame::End)
        ));
        assert!(matches!(
            serde_json::from_str::<ClientFrame>(r#"{"type":"cancel"}"#),
            Ok(ClientFrame::Cancel)
        ));
    }

    #[test]
    fn an_unknown_or_malformed_frame_is_rejected_rather_than_ignored() {
        for bad in [
            r#"{"type":"speak","text":"hi"}"#,
            r#"{"type":"text"}"#,
            r"not json",
            r"{}",
        ] {
            assert!(
                serde_json::from_str::<ClientFrame>(bad).is_err(),
                "{bad} must not parse"
            );
        }
    }

    #[test]
    fn server_frames_serialize_to_their_documented_shape() {
        let utterance =
            serde_json::to_string(&ServerFrame::Utterance { id: "utt_1".into() }).unwrap();
        assert_eq!(utterance, r#"{"type":"utterance","id":"utt_1"}"#);

        // A mark is flattened, so its fields sit beside `type` rather than
        // nested — that is what the protocol doc shows.
        let mark = serde_json::to_string(&ServerFrame::Mark {
            mark: Mark {
                start_ms: Some(120),
                end_ms: Some(380),
                start_char: Some(6),
                end_char: Some(11),
            },
        })
        .unwrap();
        assert_eq!(
            mark,
            r#"{"type":"mark","start_ms":120,"end_ms":380,"start_char":6,"end_char":11}"#
        );

        // A backend with only word timings omits the character range entirely
        // rather than sending nulls.
        let partial = serde_json::to_string(&ServerFrame::Mark {
            mark: Mark {
                start_ms: Some(0),
                end_ms: Some(40),
                start_char: None,
                end_char: None,
            },
        })
        .unwrap();
        assert_eq!(partial, r#"{"type":"mark","start_ms":0,"end_ms":40}"#);

        let progress = serde_json::to_string(&ServerFrame::Progress {
            spoken_ms: 100,
            queued_ms: 900,
        })
        .unwrap();
        assert_eq!(
            progress,
            r#"{"type":"progress","spoken_ms":100,"queued_ms":900}"#
        );

        let done = serde_json::to_string(&ServerFrame::Done {
            id: "utt_1".into(),
            chunks: 3,
        })
        .unwrap();
        assert_eq!(done, r#"{"type":"done","id":"utt_1","chunks":3}"#);

        let error = serde_json::to_string(&ServerFrame::Error {
            message: "model_not_loaded".into(),
        })
        .unwrap();
        assert_eq!(error, r#"{"type":"error","message":"model_not_loaded"}"#);
    }
}
