// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model/{model}/voice` — the voice a model speaks in.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline/voice.md`.
//!
//! The sibling of [`super::language`] and [`super::device`], addressed the same
//! way for the same reason: all three are per-`(source, model)` preferences
//! that outlive any one load, and preferences of the same shape addressed three
//! different ways is a thing a client author has to memorise rather than infer.
//!
//! This is the model's *voice*, not its language. The voice decides who speaks;
//! the language decides how the text is pronounced — which phonemizer, which
//! lexicon. They interact, since a voice trained on one language sounds wrong
//! reading another, which is why the voice list carries no language filter and
//! a picker shows both controls together.
//!
//! A voice remains a per-utterance field on `POST /speak`, and one named there
//! still wins. This is the default that field falls back to, which is what
//! every caller that has no UI for choosing — a keyboard shortcut, the applet —
//! actually gets.

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use crate::daemon::http::v1::wire::{FromDaemon, ModelVoiceState, VoiceList};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// The two paths this module serves, registered together because the
/// preference and the choices for it are one subject.
pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_model_voice, set_model_voice, clear_model_voice))
        .routes(routes!(list_model_voices))
}

/// The voice this model should speak in.
#[derive(Deserialize, utoipa::ToSchema)]
struct VoiceBody {
    /// A `voice` id in a shape the model accepts: one of its presets, a
    /// `voice:<uuid>` from the voice library, or `desc:<text>` where the model
    /// builds voices from descriptions. Empty is refused — clear the
    /// preference with `DELETE`, which is a different act.
    #[serde(default)]
    #[schema(example = "ryan")]
    voice: String,
}

#[utoipa::path(
    get,
    path = "/pipeline/{stage}/model/{model}/voice",
    tag = "pipeline",
    summary = "Read a model's voice",
    description = "\
The voice this model speaks in when an utterance names none, and what decided it: the \
stored per-model preference, or the manifest's `default_voice`.

Addressed by the stage that runs the model rather than by \"the active model\", so it \
can be read whether or not the model is loaded — that is how a card shows its voice \
before Load.

`effective` is `null` when the model has neither a stored voice nor a `default_voice`. \
That is not a quirk: a model whose voices are all cloned has no id it could fall back \
to, and speaking will be refused until one is set. A client should treat a null \
`effective` as a control the user must fill in, not as an optional one.

What the preference *may* be set to is `GET /pipeline/{stage}/model/{model}/voice/list`.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` spells it. Resolved against the backend filling this stage."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "How this model's voice resolves.", body = ModelVoiceState),
        (status = 400, description = "The stage has no backend selected, so there is nothing to resolve `model` against (`invalid_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`), or this stage's backend serves no such model (`unknown_model`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn get_model_voice(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
) -> Response {
    let source = match super::language::resolve_source(&s, stage, &model).await {
        Ok(source) => source,
        Err(r) => return *r,
    };
    let req = build_request(
        "get_model_voice",
        Some(serde_json::json!({ "source": source, "model": model })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, ModelVoiceState::from_daemon)
}

#[utoipa::path(
    post,
    path = "/pipeline/{stage}/model/{model}/voice",
    tag = "pipeline",
    summary = "Set a model's voice",
    description = "\
Stores the voice this model speaks in when an utterance names none. Kept against \
`(source, model)`, so it survives model switches and does not follow whichever model \
happens to be loaded — a voice id means nothing to another model, and one global voice \
would be wrong for every model but the one it was picked for.

A voice the model cannot resolve is refused rather than stored: a shape its \
`voice_kinds` does not include, or a preset id it does not declare. Offer what \
`GET /pipeline/{stage}/model/{model}/voice/list` returns, plus a free-text \
`desc:<text>` when the block's `kinds` includes `described`.

`POST /speak` may still name a voice for one utterance, and that wins. This is the \
default it falls back to.

The new value applies to the next utterance; one already being spoken finishes in the \
voice it started in. Clear it with `DELETE` rather than by setting an empty string.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` spells it. Resolved against the backend filling this stage."),
    ),
    request_body = VoiceBody,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Stored; this is the resulting resolution, with `source` now `override`.", body = ModelVoiceState),
        (status = 400, description = "`voice` was empty, or is not one this model can resolve (`invalid_value`), or the stage has no backend selected (`invalid_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`), or this stage's backend serves no such model (`unknown_model`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn set_model_voice(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
    axum::Json(body): axum::Json<VoiceBody>,
) -> Response {
    // Checked before the stage is resolved: an empty string is malformed
    // whatever the pipeline looks like, and it is the shape a client reaches
    // for when it means `DELETE`.
    if body.voice.is_empty() {
        return crate::daemon::http::v1::backends::json_error_msg(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "`voice` must be an id the model can resolve. To remove the preference, DELETE this path.",
        );
    }
    let source = match super::language::resolve_source(&s, stage, &model).await {
        Ok(source) => source,
        Err(r) => return *r,
    };
    let req = build_request(
        "set_model_voice",
        Some(serde_json::json!({ "source": source, "model": model, "voice": body.voice })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, ModelVoiceState::from_daemon)
}

#[utoipa::path(
    delete,
    path = "/pipeline/{stage}/model/{model}/voice",
    tag = "pipeline",
    summary = "Clear a model's voice",
    description = "\
Removes the stored voice, returning this model to its manifest `default_voice`.

Answers with the same resolution `GET` does, so a card can render the result of its own \
click without a second read — `source` will now be `default`, and `effective` will be \
`null` for a model that declares no default, which is the state where speaking is \
refused until a voice is chosen again.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` spells it. Resolved against the backend filling this stage."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Cleared; this is the resolution it fell back to.", body = ModelVoiceState),
        (status = 400, description = "The stage has no backend selected (`invalid_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`), or this stage's backend serves no such model (`unknown_model`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn clear_model_voice(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
) -> Response {
    let source = match super::language::resolve_source(&s, stage, &model).await {
        Ok(source) => source,
        Err(r) => return *r,
    };
    let req = build_request(
        "clear_model_voice",
        Some(serde_json::json!({ "source": source, "model": model })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, ModelVoiceState::from_daemon)
}

#[utoipa::path(
    get,
    path = "/pipeline/{stage}/model/{model}/voice/list",
    tag = "pipeline",
    summary = "List the voices a model can be pinned to",
    description = "\
What `POST /pipeline/{stage}/model/{model}/voice` will accept for this model: the \
presets it declares, followed by the stored cloned voices when its `voice_kinds` \
includes `cloned`.

Fill a voice picker from this rather than from the manifest alone. A model that clones \
declares no presets, so a picker built from `[[models.voices]]` would be empty for \
exactly the models that refuse to speak without a voice; and a cloned voice belongs to \
the library, not to any one model, so the manifest could not list it.

Described voices are not here — they are free text with no set to enumerate. A model \
that accepts them says so in the `kinds` of \
`GET /pipeline/{stage}/model/{model}/voice`, and a client offers a text field \
alongside this list.

An empty list with no `described` kind means the model has no voices to choose yet: \
record one on `POST /voice` first.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` spells it. Resolved against the backend filling this stage."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The voices on offer.", body = VoiceList),
        (status = 400, description = "The stage has no backend selected, so there is nothing to resolve `model` against (`invalid_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`), or this stage's backend serves no such model (`unknown_model`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn list_model_voices(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
) -> Response {
    let source = match super::language::resolve_source(&s, stage, &model).await {
        Ok(source) => source,
        Err(r) => return *r,
    };
    let req = build_request(
        "list_model_voices",
        Some(serde_json::json!({ "source": source, "model": model })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, VoiceList::from_daemon)
}
