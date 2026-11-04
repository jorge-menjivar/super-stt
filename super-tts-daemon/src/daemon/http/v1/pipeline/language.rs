// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model/{model}/language` — a model's language override.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline/language.md`.
//!
//! The sibling of [`super::device`], and addressed the same way for the same
//! reason: both are per-`(source, model)` preferences that outlive any one
//! load, and the stage is what resolves a bare model name against the backend
//! filling it. Two preferences of the same shape addressed two different ways
//! is a thing a client author has to memorise rather than infer.
//!
//! This is the model's *language*, not its voice. The language decides how text
//! is pronounced — which phonemizer, which lexicon — where the voice decides
//! who says it. They interact, since a voice trained on one language can sound
//! wrong reading another, but they are chosen separately: the voice is
//! [`super::voice`], a preference of exactly this shape, and it is also a
//! per-utterance field on `POST /speak` where this one is not.
//!
//! The symmetry with [`super::device`] goes one level further: the override and
//! the languages on offer are separate endpoints, exactly as the device
//! preference and the device list are. One answers what is set, the other what
//! can be set, and only one of them changes when the user picks a language.
//!
//! Moved here from `/backends/{source}/models/{model}/language`, which could
//! reach any installed model. Requiring the backend to be filling a stage costs
//! nothing real — a language control is only ever shown on a stage's card, so
//! the backend is selected by the time anyone asks — and a model that is
//! selected but not yet loaded still resolves, which is how that card shows its
//! language before Load.

use super::{Stage, unknown_stage};
use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use crate::daemon::http::v1::backends::{find_backend, json_error_msg};
use crate::daemon::http::v1::wire::{FromDaemon, LanguageList, ModelLanguageState, PipelineReport};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// The two paths this module serves. Registered here rather than in
/// [`super::routes`] only because the override and its list are one subject;
/// the parent merges them alongside every other `/pipeline` path.
pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(
            get_model_language,
            set_model_language,
            clear_model_language
        ))
        .routes(routes!(list_model_languages))
}

/// The language this model should speak in.
#[derive(Deserialize, utoipa::ToSchema)]
struct LanguageBody {
    /// A BCP-47 tag the model serves, such as `es-419`, or `auto` to let it
    /// pick. Empty is refused — clear an override with `DELETE`, which is a
    /// different act and answers differently.
    #[serde(default)]
    #[schema(example = "es-419")]
    language: String,
}

/// Resolve `model` against the backend filling `stage`, the same resolution an
/// omitted `source` gets on `POST /pipeline/{stage}/model`.
///
/// The daemon's language commands are keyed by `(source, model)` — that is what
/// makes an override survive a model switch — while the URL carries only the
/// model. Two backends may serve the same model name, so guessing the source by
/// scanning for whoever serves it would write one backend's preference onto
/// another's model; the stage's own selection is the only answer that cannot be
/// wrong.
///
/// `Err` carries the response to send: no such stage, a daemon that could not
/// report its pipeline, no backend selected for the stage, or a backend that
/// does not serve this model.
pub(super) async fn resolve_source(
    s: &AppState,
    stage: u32,
    model: &str,
) -> Result<String, Box<Response>> {
    if Stage::resolve(stage).is_none() {
        return Err(Box::new(unknown_stage(stage)));
    }
    let resp = dispatch(&s.daemon, build_request("get_pipeline", None)).await;
    // A failed read is the daemon's failure, not a missing backend: reporting
    // it as `invalid_backend` would send a client off to select a backend it
    // has already selected. Pass the daemon's own envelope through instead.
    if resp.status != "success" {
        return Err(Box::new(narrowed(resp, PipelineReport::from_daemon)));
    }
    let filled = resp
        .pipeline
        .as_ref()
        .and_then(|stages| stages.iter().find(|st| st.stage == stage))
        .and_then(|st| st.source.clone());
    let Some(source) = filled else {
        return Err(Box::new(json_error_msg(
            StatusCode::BAD_REQUEST,
            "invalid_backend",
            "This stage has no backend selected, so there is nothing to resolve the model against. Select one with POST /pipeline/{stage}.",
        )));
    };
    match find_backend(s, &source).await {
        Some(b) if b.models.iter().any(|m| m.name == model) => Ok(source),
        _ => Err(Box::new(json_error_msg(
            StatusCode::NOT_FOUND,
            "unknown_model",
            &format!("The backend filling this stage serves no model named `{model}`."),
        ))),
    }
}

#[utoipa::path(
    get,
    path = "/pipeline/{stage}/model/{model}/language",
    tag = "pipeline",
    summary = "Read a model's language override",
    description = "\
The language this specific model speaks in, and what decided it: the per-model \
override, the global `/settings/language` setting, or the model's own primary \
language. A card showing \"Automatic\" still has to show what automatic resolved to, \
which is why the whole resolution is returned rather than the stored value alone.

Addressed by the stage that runs the model rather than by \"the active model\", so it \
can be read whether or not the model is loaded — that is how a card shows its language \
before Load.

`override` is `null` when none is set, which is what \"follows the global setting\" \
looks like, and a monolingual model answers `multilingual: false` with the rest \
nulled. What the override *may* be set to is \
`GET /pipeline/{stage}/model/{model}/language/list`.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` spells it. Resolved against the backend filling this stage."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "How this model's language resolves.", body = ModelLanguageState),
        (status = 400, description = "The stage has no backend selected, so there is nothing to resolve `model` against (`invalid_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`), or this stage's backend serves no such model (`unknown_model`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn get_model_language(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
) -> Response {
    let source = match resolve_source(&s, stage, &model).await {
        Ok(source) => source,
        Err(r) => return *r,
    };
    let req = build_request(
        "get_model_language",
        Some(serde_json::json!({ "source": source, "model": model })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, ModelLanguageState::from_daemon)
}

#[utoipa::path(
    post,
    path = "/pipeline/{stage}/model/{model}/language",
    tag = "pipeline",
    summary = "Set a model's language override",
    description = "\
Pins this model to one language regardless of the global `/settings/language` setting. \
Stored against `(source, model)`, so it survives model switches and does not follow \
whichever model happens to be loaded.

A tag the model does not serve is refused rather than silently ignored — synthesis \
would otherwise come out in the wrong language, or not at all. Offer only what \
`GET /pipeline/{stage}/model/{model}/language/list` returns; a monolingual model \
accepts nothing at all.

The new value applies to the next utterance; one already being spoken finishes in the \
language it started in. Clear the pin with `DELETE` rather than by setting an empty \
string.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` spells it. Resolved against the backend filling this stage."),
    ),
    request_body = LanguageBody,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Override set; this is the resulting resolution, with `source` now `override`.", body = ModelLanguageState),
        (status = 400, description = "`language` was empty (`invalid_request`), this model does not serve that tag (`unsupported_language`), or the stage has no backend selected (`invalid_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`), or this stage's backend serves no such model (`unknown_model`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn set_model_language(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
    axum::Json(body): axum::Json<LanguageBody>,
) -> Response {
    // Checked before the stage is resolved: an empty string is a malformed
    // request whatever the pipeline looks like, and it is the shape a client
    // reaches for when it means `DELETE`.
    if body.language.is_empty() {
        return json_error_msg(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "`language` must be a tag the model serves, or `auto`. To remove the override, DELETE this path.",
        );
    }
    let source = match resolve_source(&s, stage, &model).await {
        Ok(source) => source,
        Err(r) => return *r,
    };
    let req = build_request(
        "set_model_language",
        Some(serde_json::json!({ "source": source, "model": model, "language": body.language })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, ModelLanguageState::from_daemon)
}

#[utoipa::path(
    delete,
    path = "/pipeline/{stage}/model/{model}/language",
    tag = "pipeline",
    summary = "Clear a model's language override",
    description = "\
Removes the per-model pin, returning this model to Automatic: the global \
`/settings/language` setting, or the model's own primary language when that is unset \
too.

Answers with the same resolution `GET` does, so a card can render the result of its \
own click without a second read — `source` will now be `global` or `default`.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` spells it. Resolved against the backend filling this stage."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Override cleared; this is the resolution it fell back to.", body = ModelLanguageState),
        (status = 400, description = "The stage has no backend selected (`invalid_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`), or this stage's backend serves no such model (`unknown_model`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn clear_model_language(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
) -> Response {
    let source = match resolve_source(&s, stage, &model).await {
        Ok(source) => source,
        Err(r) => return *r,
    };
    let req = build_request(
        "clear_model_language",
        Some(serde_json::json!({ "source": source, "model": model })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, ModelLanguageState::from_daemon)
}

#[utoipa::path(
    get,
    path = "/pipeline/{stage}/model/{model}/language/list",
    tag = "pipeline",
    summary = "List the languages a model can be pinned to",
    description = "\
What `POST /pipeline/{stage}/model/{model}/language` will accept for this model: the \
tags it serves, plus the reserved `auto` for letting it pick.

Fill a language picker from this rather than from a general BCP-47 list — a tag the \
model does not serve is refused, and offering one is an error the user only discovers \
by choosing it, after which the utterance they wanted comes out mispronounced or not \
at all.

Empty for a monolingual model, which has nothing to choose however many tags its \
manifest lists. That is the same shape `GET /pipeline/{stage}/model/{model}/device/list` \
answers with for a model that runs remotely, and a client hides the control on an empty \
list rather than special-casing a status.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` spells it. Resolved against the backend filling this stage."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The languages on offer.", body = LanguageList),
        (status = 400, description = "The stage has no backend selected, so there is nothing to resolve `model` against (`invalid_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`), or this stage's backend serves no such model (`unknown_model`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn list_model_languages(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
) -> Response {
    let source = match resolve_source(&s, stage, &model).await {
        Ok(source) => source,
        Err(r) => return *r,
    };
    let req = build_request(
        "list_model_languages",
        Some(serde_json::json!({ "source": source, "model": model })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, LanguageList::from_daemon)
}
