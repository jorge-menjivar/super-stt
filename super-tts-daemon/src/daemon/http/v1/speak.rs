// SPDX-License-Identifier: GPL-3.0-only
//! `POST /v1/speak` and `POST /v1/speak/stop` — synthesize text and play it.
//!
//! Contract: `docs/protocol/endpoints/v1/speak.md` and
//! `docs/protocol/endpoints/v1/speak/stop.md`.
//!
//! The endpoint is deliberately thin: it validates the envelope, hands the body
//! to the command dispatcher, and maps the result. Everything that decides how
//! speech happens lives in [`crate::daemon::speech`].
//!
//! Text that is still being written — an LLM reply spoken as it arrives — goes
//! to [`super::speak_stream`] instead; this pair takes complete text.

use crate::daemon::http::internal::helpers::dispatch::{
    build_request, dispatch, json_response, narrowed,
};
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use super_tts_shared::models::protocol::{DaemonResponse, ErrorCode};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// Fields `POST /speak` used to take, and no longer does.
///
/// Each one chose how an utterance was spoken — which voice, which language,
/// how fast, in what manner. Those are the user's settings: a voice and a
/// language are set through `/pipeline/{stage}/model/{model}/voice` and
/// `/settings/language`, and the daemon reads them per utterance. An app that
/// can make the machine talk does not thereby get a say in which voice it talks
/// in, so there is nowhere in a speak request to say it.
const REMOVED_FIELDS: &[&str] = &["voice", "language", "speed", "instructions"];

/// The removed fields `body` still names.
///
/// A field present but `null` does not count: that is how a client spells "use
/// what is configured", which is what the request means now anyway.
fn removed_fields_named(body: &Value) -> Vec<&'static str> {
    REMOVED_FIELDS
        .iter()
        .copied()
        .filter(|field| body.get(*field).is_some_and(|v| !v.is_null()))
        .collect()
}

/// Refuse a request that still carries one of [`REMOVED_FIELDS`].
///
/// Refused rather than ignored, and this is the whole point of the function: a
/// client that asks for a voice and is silently given a different one has no
/// way to find out. A `400` naming the field and where the setting now lives
/// turns a wrong-voice mystery into a one-line fix.
fn refuse_removed_fields(body: &Value) -> Option<Response> {
    let named = removed_fields_named(body);
    if named.is_empty() {
        return None;
    }
    let message = format!(
        "{} {} no longer accepted on /speak; how an utterance is spoken is \
         configuration — set a voice with POST /pipeline/{{stage}}/model/{{model}}/voice \
         and a language with POST /settings/language",
        named.join(", "),
        if named.len() == 1 { "is" } else { "are" },
    );
    Some(
        json_response(&DaemonResponse::error_with_code(
            ErrorCode::InvalidValue,
            &message,
        ))
        .into_response(),
    )
}

/// The body of `POST /speak`.
///
/// The handler parses a bare [`Value`] rather than this type, because the
/// dispatcher re-reads the same object and going through a typed struct would
/// mean maintaining two spellings of the same fields. This is what that body's
/// shape *is*, published in the `OpenAPI` document and kept beside the handler
/// that validates it.
#[derive(Deserialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct SpeakBody {
    /// What to speak, and the whole of the request. Empty or whitespace-only is
    /// `400 invalid_value`, and the daemon caps it at 50 000 characters — a
    /// model may declare a lower cap of its own in its manifest.
    ///
    /// There is deliberately nothing else here. Which voice, which language and
    /// how fast are settings the user owns; see `REMOVED_FIELDS`.
    #[schema(example = "The kettle is boiling.")]
    pub(crate) text: String,
}

/// `POST /speak` — the utterance was accepted and is being played out.
#[derive(Serialize, ToSchema)]
pub(crate) struct UtteranceAccepted {
    /// Always `success`.
    #[schema(example = "success")]
    status: &'static str,
    /// Names this utterance on the `speaking_state` and `speech_progress`
    /// event topics, and is what `POST /speak/stop` reports having cancelled.
    #[schema(example = "utt_9f2c1e40")]
    utterance_id: String,
}

/// `POST /speak/stop` — what, if anything, was stopped.
#[derive(Serialize, ToSchema)]
pub(crate) struct UtteranceStopped {
    /// Always `success`, whether or not anything was speaking.
    #[schema(example = "success")]
    status: &'static str,
    /// The utterance that was cancelled. **Absent when nothing was speaking** —
    /// use its presence, not the status, to tell the two apart.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "utt_9f2c1e40")]
    utterance_id: Option<String>,
}

#[utoipa::path(
    post,
    path = "/speak",
    tag = "speak",
    summary = "Speak a piece of text",
    description = "\
Synthesizes `text` with the loaded model and plays it on the daemon's output \
device.

`202`, not `200`: the call returns once the audio is *queued*, and the device is \
still playing it out when you read the response. To follow the utterance to its \
end, subscribe to `speaking_state` and `speech_progress` on `GET /events` with the \
`playback_events` scope — those outlive any one request, which is what a status \
widget actually needs.

**One utterance at a time, newest wins.** Speaking while something is already \
playing interrupts it: that is what \"speak this instead\" means, and it is why \
this endpoint is not refused while the daemon is busy. Only swapping the model out \
from under live synthesis is refused, by `POST /pipeline/{stage}/model`.

The daemon normalizes `text` first — markup that would otherwise be read out \
literally is stripped, and long text is split on sentence boundaries so playback \
starts before the whole thing is synthesized. Numbers, dates and currency are \
passed through untouched, because a wrong number-to-words is worse than none.

**`text` is the whole request.** How an utterance is spoken — which voice, which \
language, how fast, in what manner — is the user's configuration, read per \
utterance by the daemon. Set a voice with \
`POST /pipeline/{stage}/model/{model}/voice` and a language with \
`POST /settings/language`, both under the `settings` scope. An app that can make \
the machine talk does not thereby get a say in which voice it talks in.

`voice`, `language`, `speed` and `instructions` used to be accepted here. A request \
still carrying one is refused with `400 invalid_value` naming it, rather than \
ignored — a client given a different voice than it asked for has no way to find out.

For text that is still being generated, use `GET /speak/stream` instead.",
    request_body(content = SpeakBody, description = "`text`, and nothing else. Voice, language, speed and instructions are settings, not request fields."),
    // `speak` is what the router enforces; `settings` is required only for a
    // request that names an override field, which is a body-dependent check the
    // handler makes. Declaring it here would tell every client it needs a scope
    // it usually does not — see the description and the 403 below.
    security(("session_token" = ["speak"])),
    responses(
        (status = 202, description = "Queued, and playing out. `utterance_id` names it.", body = UtteranceAccepted),
        (status = 400, description = "`text` missing, empty, or over the cap; a request still carrying the removed `voice`/`language`/`speed`/`instructions` fields; or a malformed body.", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `speak` scope.", body = ErrorEnvelope),
        (status = 409, description = "No model is loaded (`model_not_loaded`). Load one with `POST /pipeline/1/model` and retry.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 500, description = "The backend failed, or the output device could not be opened. `message` carries the reason.", body = ErrorEnvelope),
    ),
)]
/// `POST /v1/speak` — synthesize `text` with the active model and play it.
///
/// Answers `202 Accepted` with the utterance id: the request returns once the
/// audio is queued, while the device is still playing it out. The id is what a
/// client cancels by, and it is returned from the first version of this
/// endpoint even though the current policy is "one at a time, newest wins" —
/// adding an id later would break every client written against it.
pub(crate) async fn speak(
    State(state): State<AppState>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let Some(axum::Json(body)) = body else {
        return json_response(&DaemonResponse::error_with_code(
            ErrorCode::InvalidValue,
            "missing request body",
        ))
        .into_response();
    };
    // `text` is checked here as well as in the dispatcher so a missing field is
    // a coded 400 rather than the dispatcher's bare parse message.
    //
    // Before the removed-field check on purpose: "is this a valid utterance at
    // all" precedes "you are spelling it the old way".
    if body.get("text").and_then(Value::as_str).is_none() {
        return json_response(&DaemonResponse::error_with_code(
            ErrorCode::InvalidValue,
            "missing text",
        ))
        .into_response();
    }
    if let Some(refusal) = refuse_removed_fields(&body) {
        return refusal;
    }

    let request = build_request("speak", Some(body));
    let response = dispatch(&state.daemon, request).await;
    if response.status != "success" {
        return json_response(&response).into_response();
    }
    // `handle_speak` sets `utterance_id` on every success; the fallback keeps
    // the endpoint answering its own shape rather than a partial one if a
    // future command path ever forgets.
    let accepted = UtteranceAccepted {
        status: "success",
        utterance_id: response.utterance_id.unwrap_or_default(),
    };
    (StatusCode::ACCEPTED, axum::Json(accepted)).into_response()
}

#[utoipa::path(
    post,
    path = "/speak/stop",
    tag = "speak",
    summary = "Stop the utterance now playing",
    description = "\
Cancels in-flight synthesis, drops whatever audio is queued behind it, and flushes \
the output ring. The daemon then publishes `speaking_state` with \
`is_speaking: false` to `GET /events` subscribers.

**Idempotent, and never an error.** Stopping when nothing is speaking succeeds: a \
client cancelling on a keypress should not have to win a race to get a clean \
result, nor distinguish \"I stopped it\" from \"it had already finished\" to avoid \
showing the user an error. `utterance_id` is present only in the first case.

Unlike closing a connection, this works regardless of who started the utterance. \
`POST /speak` returns as soon as the audio is queued, so by the time a user reaches \
for a stop button there is no connection left to close — which is the whole reason \
this endpoint exists.",
    security(("session_token" = ["speak"])),
    responses(
        (status = 200, description = "Stopped, or nothing was speaking.", body = UtteranceStopped),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `speak` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
/// `POST /v1/speak/stop` — stop the current utterance and drop queued audio.
///
/// Succeeds whether or not anything was speaking: a client cancelling on a
/// keypress should not have to know whether it won the race.
pub(crate) async fn speak_stop(State(state): State<AppState>) -> Response {
    let request = build_request("stop_speaking", None);
    let response = dispatch(&state.daemon, request).await;
    narrowed(response, |r| UtteranceStopped {
        status: "success",
        utterance_id: r.utterance_id,
    })
}

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(speak))
        .routes(routes!(speak_stop))
}
