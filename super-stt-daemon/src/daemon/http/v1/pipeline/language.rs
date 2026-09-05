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
//! Every stage answers it. A post-processor is monolingual and says so in
//! `multilingual`, which is a real answer rather than an error — the point of
//! addressing stages by position is that they answer the same verbs.
//!
//! The symmetry with [`super::device`] goes one level further: the override and
//! the languages on offer are separate endpoints, exactly as the device
//! preference and the device list are. One answers what is set, the other what
//! can be set.
use super::{Stage, unknown_stage};
use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use crate::daemon::http::v1::backends::json_error;
use crate::daemon::http::v1::wire::{FromDaemon, LanguageList, ModelLanguageState};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(
            get_model_language,
            set_model_language,
            clear_model_language
        ))
        .routes(routes!(list_model_languages))
}

/// The language this model should transcribe in.
#[derive(Deserialize, utoipa::ToSchema)]
struct LanguageBody {
    /// A BCP-47 tag such as `es`, or `auto` to let the model detect it. Empty is
    /// refused — clear an override with `DELETE`.
    #[serde(default)]
    #[schema(example = "es")]
    language: String,
}

/// Resolve `model` against the backend filling `stage`, the same resolution an
/// omitted `source` gets on `POST /pipeline/{stage}/model`.
///
/// `Err` carries the response to send: no such stage, no backend selected for
/// it, or a backend that does not serve this model.
async fn resolve_source(s: &AppState, stage: u32, model: &str) -> Result<String, Box<Response>> {
    if Stage::resolve(stage).is_none() {
        return Err(Box::new(unknown_stage(stage)));
    }
    let one = dispatch(&s.daemon, build_request("get_pipeline", None))
        .await
        .pipeline
        .as_ref()
        .and_then(|stages| stages.iter().find(|st| st.stage == stage).cloned());
    let Some(source) = one.and_then(|st| st.source) else {
        return Err(Box::new(json_error(
            StatusCode::BAD_REQUEST,
            "invalid_backend",
        )));
    };
    match crate::daemon::http::v1::backends::find_backend(s, &source).await {
        Some(b) if b.models.iter().any(|m| m.name == model) => Ok(source),
        _ => Err(Box::new(json_error(StatusCode::NOT_FOUND, "unknown_model"))),
    }
}

#[utoipa::path(
    get,
    path = "/pipeline/{stage}/model/{model}/language",
    tag = "pipeline",
    summary = "Read a model's language override",
    description = "\
The language this specific model transcribes in, and what decided it: the per-model \
override, the global `/settings/language` setting, or the model's own default. \
Addressed by `(source, model)` rather than \"the active model\", so it can be read \
whether or not the model is loaded.

`override` is `null` when none is set, which is what \"follows the global setting\" \
looks like. What the override *may* be set to is \
`GET /pipeline/{stage}/model/{model}/language/list`.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
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
Pins this model to a language regardless of the global `/settings/language` setting. A tag \
the model does not serve is refused rather than silently ignored.

Overridden in turn by a `language` field in a single `POST /transcribe` body.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` spells it. Resolved against the backend filling this stage."),
    ),
    request_body = LanguageBody,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Override set.", body = ModelLanguageState),
        (status = 400, description = "The body was empty (`invalid_request`), this model does not serve that language (`unsupported_language`), or the stage has no backend selected (`invalid_backend`).", body = ErrorEnvelope),
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
    if body.language.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "invalid_request");
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
Removes the per-model pin, returning this model to the global `/settings/language` setting.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` spells it. Resolved against the backend filling this stage."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Override cleared.", body = ModelLanguageState),
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
tags it serves, plus the reserved `auto` for letting it detect the language itself.

Fill a language picker from this rather than from a general BCP-47 list — a tag the \
model does not serve is refused, and offering one is an error the user only discovers \
by choosing it.

Empty for a monolingual model, which has nothing to choose however many tags its \
manifest lists. That is the same shape `GET /pipeline/{stage}/model/{model}/device/list` \
answers with for a model that runs remotely, and a client hides the control on an empty \
list rather than special-casing a status.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
        ("model" = String, Path, description = "The model\'s name, as `GET /pipeline/{stage}/model/list` spells it. Resolved against the backend filling this stage."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The languages on offer.", body = LanguageList),
        (status = 400, description = "The stage has no backend selected, so there is nothing to resolve `model` against (`invalid_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`), or this stage\'s backend serves no such model (`unknown_model`).", body = ErrorEnvelope),
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
