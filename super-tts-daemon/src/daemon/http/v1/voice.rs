// SPDX-License-Identifier: GPL-3.0-only
//! `/v1/voice` — the cloned-voice library.
//!
//! Contract: `docs/protocol/endpoints/v1/voice.md`.
//!
//! A voice is a reference recording plus the id (`voice:<uuid>`) that names it
//! on [`POST /speak`](super::speak). These endpoints own the recording; the
//! speak path is what hands it to a backend, the first time a loaded model is
//! asked for that voice.
//!
//! **The clip is uploaded as a raw body, not JSON.** A recording is megabytes
//! of PCM; base64 in a JSON envelope would inflate it by a third and push it
//! through a parser that exists for small objects. Metadata rides in the query
//! string instead, which keeps adding a voice to one request.
//!
//! Its own scope rather than part of `settings`: these are recordings of a
//! person, and an app that manages models has no business reading them.
//!
//! The house `json_error` envelope builders are shared with the backends
//! endpoints, where they were first written; they are house style, not
//! backend-specific.

use super::backends::{json_error, json_error_msg};
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use crate::voices::VoiceError;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use super_tts_shared::models::voices::{
    VoiceDeletedResponse, VoiceInfo, VoiceListResponse, VoiceModelSupport, VoiceResponse,
};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// The five voice-library paths.
///
/// `POST /voice` is merged in from a router of its own so its body limit lands
/// on that one route. Layering the whole group would raise the ceiling on the
/// metadata endpoints too, and those parse small JSON objects — the global
/// default is exactly the guard they want.
pub(crate) fn routes() -> OpenApiRouter<AppState> {
    let upload = OpenApiRouter::new()
        .routes(routes!(create_voice))
        // The upload body is bounded here rather than by the global default:
        // the default exists to stop a runaway JSON object, and a voice
        // recording is legitimately far larger than one.
        .layer(DefaultBodyLimit::max(crate::voices::MAX_UPLOAD_BYTES));

    OpenApiRouter::new()
        .routes(routes!(list_voices))
        .merge(upload)
        .routes(routes!(get_voice, rename_voice, delete_voice))
        .routes(routes!(voice_audio))
}

/// Query parameters of `POST /voice`.
///
/// `label` is optional to `serde` but required by the library, so a request
/// that omits it is refused with the house error envelope rather than axum's
/// bare rejection text.
#[derive(Debug, Deserialize)]
pub(crate) struct NewVoice {
    /// Display name. A library of unnamed voices is unusable.
    #[serde(default)]
    label: Option<String>,
    /// What the clip says, for models that clone in-context.
    #[serde(default)]
    transcript: Option<String>,
}

/// Body of `PATCH /voice/{id}`.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct RenameVoice {
    /// The new display name. Trimmed; empty is `400 invalid_value`, and it is
    /// capped at 128 characters.
    #[schema(example = "Ada Lovelace")]
    label: String,
}

#[utoipa::path(
    get,
    path = "/voice/list",
    tag = "voices",
    summary = "List the stored voices",
    description = "\
Every voice in the library, newest first, alongside what the currently loaded model \
can do with them.

`voice_id` on each entry is the string to pass as `voice` on `POST /speak`; `id` is \
the same value without its prefix, for building the paths under `/voice`.

`model` is the answer to the three questions a voices UI has — whether to offer \
cloning at all (`clones`), how much of a recording will actually be used \
(`clone_ref_seconds`), and whether to ask the user what the clip says \
(`needs_transcript`). It is `null` when nothing is loaded: the library is still \
readable, it just cannot be spoken with yet. The capability rides on this listing \
rather than on the model catalog because this is where it is needed; the stored \
voices themselves are model-independent and do not change when the user switches \
models.

A daemon that has never cloned a voice answers `200` with an empty list.",
    security(("session_token" = ["voices"])),
    responses(
        (status = 200, description = "The stored voices, and the loaded model's cloning capability.", body = VoiceListResponse),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `voices` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "The library could not be read (`voice_library_unavailable`).", body = ErrorEnvelope),
    ),
)]
/// `GET /voice/list` — every stored voice, newest first, alongside what the
/// loaded model can do with them.
///
/// The capability rides on this listing rather than on the model catalog
/// because it is what a voices UI needs and nothing else does: whether the page
/// should offer cloning at all, how long a recording is worth keeping, and
/// whether to ask the user for a transcript. The model list returns
/// `(name, source)` pairs and gaining a voice vocabulary there is a larger
/// change than cloning needs.
pub(crate) async fn list_voices(State(s): State<AppState>) -> Response {
    let voices = match s.daemon.voices.list() {
        Ok(voices) => voices,
        Err(e) => return error(&e),
    };
    let body = VoiceListResponse {
        status: "success".into(),
        voices: voices.iter().map(describe).collect(),
        model: active_model_support(&s).await,
    };
    ok(&body)
}

/// What the loaded model can do with a cloned voice. `None` when nothing is
/// loaded — the library is still readable, it just cannot be spoken with yet.
async fn active_model_support(s: &AppState) -> Option<VoiceModelSupport> {
    use super_tts_registry_types::manifest::VoiceKind;

    let guard = s.daemon.model.read().await;
    let def = &guard.as_ref()?.definition;
    Some(VoiceModelSupport {
        name: def.name.clone(),
        source: def.source.clone(),
        clones: def.product.voice_kinds.contains(&VoiceKind::Cloned),
        clone_ref_seconds: def.product.clone_ref_seconds,
        needs_transcript: def.product.clone_needs_transcript,
    })
}

#[utoipa::path(
    post,
    path = "/voice",
    tag = "voices",
    summary = "Add a voice from a WAV upload",
    description = "\
Stores a reference recording and returns the voice that now names it.

**The request body is the WAV file itself**, not JSON — a recording is megabytes of \
PCM, and base64 in a JSON envelope would inflate it by a third and push it through a \
parser meant for small objects. The metadata rides in the query string, which keeps \
adding a voice to one request.

Every upload is converted to one canonical shape — mono 16-bit PCM at 24 kHz — so \
backends receive a predictable format and need no resampler of their own. \
Multi-channel input is averaged down rather than having a channel picked, and \
anything at another rate is resampled. WAV only: decoding compressed formats would \
put a media parser inside a daemon that runs for the whole login session.

The clip is capped at 120 seconds and 48 MiB, the label at 128 characters and the \
transcript at 4000. A model's own `clone_ref_seconds` is applied on top when the \
clip is handed over, so the whole recording is kept and switching models never asks \
the user to record again.

`201`, not `200`: a new resource exists at `/voice/{id}`.",
    params(
        ("label" = String, Query,
         description = "Display name. Trimmed; empty is `400 invalid_value`.",
         example = "Ada"),
        ("transcript" = Option<String>, Query,
         description = "What the clip says. Needed only by models that declare `clone_needs_transcript`, which condition on the words as well as the audio — supply it when you have it, since a model that needs it cannot use a voice stored without it.",
         example = "The quick brown fox jumps over the lazy dog."),
    ),
    request_body(content = String, content_type = "audio/wav",
                 description = "The WAV file, as the raw body."),
    security(("session_token" = ["voices"])),
    responses(
        (status = 201, description = "Stored. `voice.voice_id` is what `POST /speak` takes as `voice`.", body = VoiceResponse),
        (status = 400, description = "Missing or empty `label`, an over-long label or transcript, an empty body (`invalid_value`); a body that is not a readable WAV (`unsupported_audio`); or a recording longer than 120 s (`clip_too_long`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `voices` scope.", body = ErrorEnvelope),
        (status = 413, description = "The upload exceeded 48 MiB and was rejected before it was read.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "The library could not be written (`voice_library_unavailable`).", body = ErrorEnvelope),
    ),
)]
/// `POST /voice?label=…[&transcript=…]` — add a voice from a WAV upload.
///
/// Answers `201` with the created voice, whose `voice_id` is what `POST /speak`
/// takes as `voice`.
pub(crate) async fn create_voice(
    State(s): State<AppState>,
    Query(params): Query<NewVoice>,
    body: axum::body::Bytes,
) -> Response {
    if body.is_empty() {
        return json_error_msg(
            StatusCode::BAD_REQUEST,
            "invalid_value",
            "request body is empty; POST the WAV file as the body",
        );
    }
    let library = Arc::clone(&s.daemon.voices);
    // Decoding and resampling a two-minute clip is real CPU work, and it must
    // not run on the runtime thread that also drives playback.
    let created = tokio::task::spawn_blocking(move || {
        library.create(
            &body,
            params.label.as_deref().unwrap_or_default(),
            params.transcript.as_deref(),
        )
    })
    .await;

    match created {
        Ok(Ok(voice)) => {
            // Hand the clip to the loaded model now, in the background. The
            // first use of a cloned voice is what pays to derive it — a speaker
            // embedding, and for a clip with a transcript the codec codes of an
            // in-context example — and that bill is seconds to tens of seconds
            // of GPU work on a clip length nothing has encoded before. Paid
            // here it overlaps with the user reading the library they just
            // added to; paid on first use it lands on a Preview button that
            // then looks broken.
            //
            // Spawned rather than awaited: the recording is safely stored the
            // moment `create` returns, and making the save wait for a GPU would
            // trade one stall for another. Nothing reads the result — the
            // registration either warmed the cache or it did not, and speaking
            // registers properly either way.
            let daemon = Arc::clone(&s.daemon);
            let voice_id = voice.voice_id();
            tokio::spawn(async move {
                daemon
                    .speech
                    .prepare_cloned_voice(&daemon.model, &voice_id)
                    .await;
            });
            (
                StatusCode::CREATED,
                [("content-type", "application/json")],
                encode(&one(&voice)),
            )
                .into_response()
        }
        Ok(Err(e)) => error(&e),
        Err(e) => json_error_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            &format!("storing the clip failed: {e}"),
        ),
    }
}

#[utoipa::path(
    get,
    path = "/voice/{id}",
    tag = "voices",
    summary = "Read one voice's metadata",
    description = "\
The same entry `GET /voice/list` carries, for a single voice — what a client refetches \
after a rename rather than re-listing the library.

`{id}` is accepted with or without the `voice:` prefix. An id that names nothing is \
`404 not_found`, including when it is not a uuid at all, so a malformed id tells a \
caller nothing about the filesystem.",
    params(("id" = String, Path,
            description = "The voice's uuid, with or without the `voice:` prefix.",
            example = "2f8a2d0e-9c31-4e77-b0aa-1c6b2f0a51d4")),
    security(("session_token" = ["voices"])),
    responses(
        (status = 200, description = "The voice.", body = VoiceResponse),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `voices` scope.", body = ErrorEnvelope),
        (status = 404, description = "No voice with that id (`not_found`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "The library could not be read (`voice_library_unavailable`).", body = ErrorEnvelope),
    ),
)]
/// `GET /voice/{id}` — one voice's metadata.
pub(crate) async fn get_voice(State(s): State<AppState>, Path(id): Path<String>) -> Response {
    match s.daemon.voices.get(&id) {
        Ok(voice) => ok(&one(&voice)),
        Err(e) => error(&e),
    }
}

#[utoipa::path(
    patch,
    path = "/voice/{id}",
    tag = "voices",
    summary = "Rename a stored voice",
    description = "\
Changes the display name, and nothing else.

The transcript is deliberately not editable. It describes the stored audio, and a \
backend caches what it derived from the pair — changing it would leave every \
registration already made disagreeing with the library. To change what the clip says, \
add a new voice.

Answers with the voice as it now stands.",
    params(("id" = String, Path,
            description = "The voice's uuid, with or without the `voice:` prefix.",
            example = "2f8a2d0e-9c31-4e77-b0aa-1c6b2f0a51d4")),
    request_body = RenameVoice,
    security(("session_token" = ["voices"])),
    responses(
        (status = 200, description = "Renamed; this is the voice as it now stands.", body = VoiceResponse),
        (status = 400, description = "`label` missing, empty, or over 128 characters (`invalid_value`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `voices` scope.", body = ErrorEnvelope),
        (status = 404, description = "No voice with that id (`not_found`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "The library could not be written (`voice_library_unavailable`).", body = ErrorEnvelope),
    ),
)]
/// `PATCH /voice/{id}` — rename.
///
/// Only the label. The transcript describes the stored audio and backends
/// cache what they derived from the pair, so changing it would leave every
/// registration already made disagreeing with the library.
pub(crate) async fn rename_voice(
    State(s): State<AppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<RenameVoice>,
) -> Response {
    match s.daemon.voices.rename(&id, &body.label) {
        Ok(voice) => ok(&one(&voice)),
        Err(e) => error(&e),
    }
}

#[utoipa::path(
    delete,
    path = "/voice/{id}",
    tag = "voices",
    summary = "Forget a voice and its recording",
    description = "\
Removes the stored clip and its metadata. `deleted` echoes the wire id that is gone.

If the model loaded at the time holds the voice, the daemon also asks the backend to \
release it. That release is best-effort: the registration lives in the backend's \
memory and dies with the instance, so a failure costs a little memory until the next \
model switch, never correctness. The voice is deleted either way, and the request \
still succeeds — failing it over the cleanup would tell the user the wrong thing.

An utterance already in flight in this voice is not interrupted.",
    params(("id" = String, Path,
            description = "The voice's uuid, with or without the `voice:` prefix.",
            example = "2f8a2d0e-9c31-4e77-b0aa-1c6b2f0a51d4")),
    security(("session_token" = ["voices"])),
    responses(
        (status = 200, description = "Deleted.", body = VoiceDeletedResponse),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `voices` scope.", body = ErrorEnvelope),
        (status = 404, description = "No voice with that id (`not_found`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "The library could not be written (`voice_library_unavailable`).", body = ErrorEnvelope),
    ),
)]
/// `DELETE /voice/{id}` — forget a voice and its recording.
pub(crate) async fn delete_voice(State(s): State<AppState>, Path(id): Path<String>) -> Response {
    let voice = match s.daemon.voices.get(&id) {
        Ok(v) => v,
        Err(e) => return error(&e),
    };
    if let Err(e) = s.daemon.voices.delete(&id) {
        return error(&e);
    }
    release_from_loaded_model(&s, &voice.voice_id()).await;
    ok(&VoiceDeletedResponse {
        status: "success".into(),
        deleted: voice.voice_id(),
    })
}

#[utoipa::path(
    get,
    path = "/voice/{id}/audio",
    tag = "voices",
    summary = "Download a voice's reference clip",
    description = "\
The stored recording, so a client can let the user hear what a voice was built from.

The response body is `audio/wav`, not JSON: mono 16-bit PCM at 24 kHz — the canonical \
form the daemon stores, not the bytes that were uploaded. Errors on this path still \
answer with the house JSON envelope.

This is the archive copy, whole. A model receives only as much of it as its \
`clone_ref_seconds` allows, which is why the library keeps the full recording.",
    params(("id" = String, Path,
            description = "The voice's uuid, with or without the `voice:` prefix.",
            example = "2f8a2d0e-9c31-4e77-b0aa-1c6b2f0a51d4")),
    security(("session_token" = ["voices"])),
    responses(
        (status = 200, description = "The stored clip: mono 16-bit PCM WAV at 24 kHz.",
         content_type = "audio/wav"),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `voices` scope.", body = ErrorEnvelope),
        (status = 404, description = "No voice with that id (`not_found`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "The library could not be read (`voice_library_unavailable`).", body = ErrorEnvelope),
    ),
)]
/// `GET /voice/{id}/audio` — the stored clip, so a client can play it back.
pub(crate) async fn voice_audio(State(s): State<AppState>, Path(id): Path<String>) -> Response {
    let library = Arc::clone(&s.daemon.voices);
    match tokio::task::spawn_blocking(move || library.clip_wav(&id)).await {
        Ok(Ok(bytes)) => (
            StatusCode::OK,
            [("content-type", "audio/wav")],
            axum::body::Bytes::from(bytes),
        )
            .into_response(),
        Ok(Err(e)) => error(&e),
        Err(e) => json_error_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            &format!("reading the clip failed: {e}"),
        ),
    }
}

/// Tell the loaded model to drop a voice the user just deleted.
///
/// Best-effort by design: the registration lives in the backend's memory and
/// dies with the instance, so a failure here costs a little VRAM until the next
/// model switch — not correctness. Deleting the voice has already succeeded,
/// and failing the request over the cleanup would tell the user the wrong
/// thing.
async fn release_from_loaded_model(s: &AppState, voice_id: &str) {
    let guard = s.daemon.model.read().await;
    let Some(loaded) = guard.as_ref() else {
        return;
    };
    if !loaded.cloned_voices.lock().remove(voice_id) {
        return;
    }
    if let Err(e) = loaded.instance.unregister_voice(voice_id).await {
        log::warn!("backend kept cloned voice {voice_id} after deletion: {e}");
    }
}

/// One stored voice as the wire type: its metadata plus the id clients pass to
/// `/speak`, so nothing downstream has to know how to build it.
///
/// The library's own struct is the on-disk format and is converted rather than
/// serialized directly — the file may gain fields the protocol does not want,
/// and the protocol may gain fields the file has no business storing.
fn describe(voice: &crate::voices::Voice) -> VoiceInfo {
    VoiceInfo {
        id: voice.id.clone(),
        voice_id: voice.voice_id(),
        label: voice.label.clone(),
        transcript: voice.transcript.clone(),
        created_at: voice.created_at.clone(),
        duration_seconds: voice.duration_seconds,
        sample_rate: voice.sample_rate,
        channels: voice.channels,
    }
}

/// The single-voice response body.
fn one(voice: &crate::voices::Voice) -> VoiceResponse {
    VoiceResponse {
        status: "success".into(),
        voice: describe(voice),
    }
}

/// Serialize a response body. A failure here is not reachable for these shapes
/// — every field is a string, number, bool, or `Vec` of the same — so it
/// degrades to the house error envelope rather than propagating an error no
/// caller could act on.
fn encode<T: Serialize>(body: &T) -> String {
    serde_json::to_string(body).unwrap_or_else(|e| {
        log::error!("serializing a voice response failed: {e}");
        String::from(r#"{"status":"error","error_code":"internal"}"#)
    })
}

/// House-style JSON success response at `200`.
fn ok<T: Serialize>(body: &T) -> Response {
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        encode(body),
    )
        .into_response()
}

/// Map a library error onto the house error envelope.
fn error(e: &VoiceError) -> Response {
    match e {
        VoiceError::InvalidValue(msg) => {
            json_error_msg(StatusCode::BAD_REQUEST, "invalid_value", msg)
        }
        VoiceError::Audio(crate::voices::wav::WavError::TooLong { .. }) => {
            json_error_msg(StatusCode::BAD_REQUEST, "clip_too_long", &e.to_string())
        }
        VoiceError::Audio(_) => {
            json_error_msg(StatusCode::BAD_REQUEST, "unsupported_audio", &e.to_string())
        }
        VoiceError::NotFound => json_error(StatusCode::NOT_FOUND, "not_found"),
        VoiceError::Io(_) => json_error_msg(
            StatusCode::SERVICE_UNAVAILABLE,
            "voice_library_unavailable",
            &e.to_string(),
        ),
    }
}
