// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model` — the model running in a stage.
//!
//! Run one, stop it, abandon a load still in flight, or re-instantiate it in
//! place. All four are scoped to the stage in the path: the stages provision
//! independently, so one stage's cancel must not abandon another's download.

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Deserialize;

use super::{Stage, unknown_stage};
use crate::daemon::http::v1::wire::{FromDaemon, ModelList, StageMutation};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};

/// Which model to run in the stage.
#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct SetModelBody {
    /// The model's name.
    #[schema(example = "whisper-tiny")]
    pub(crate) model: String,
    /// The backend serving it. Omitted resolves to the backend already selected
    /// for this stage.
    #[serde(default)]
    pub(crate) source: Option<String>,
}

/// `POST /pipeline/{stage}/model` — run a model in this stage.
#[utoipa::path(
    post,
    path = "/pipeline/{stage}/model",
    tag = "pipeline",
    summary = "Run a model in a stage",
    description = "\
Loads `model` into the stage, downloading it first if this machine does not have it \
yet. That download can be long: watch the `download_progress` event topic, or poll \
`GET /pipeline/{stage}` and read `switch`. Abandon it with \
`POST /pipeline/{stage}/model/cancel`.

Omitting `source` uses the backend already selected for the stage.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
    ),
    request_body = SetModelBody,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Accepted. The model may still be downloading — read `switch` to follow it.", body = StageMutation),
        (status = 400, description = "No such model (`invalid_model`), or no such backend (`invalid_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn set_stage_model(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
    axum::Json(body): axum::Json<SetModelBody>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    let mut data = serde_json::json!({ "model": body.model });
    if let Some(source) = body.source {
        data["source"] = serde_json::Value::String(source);
    }
    let resp = dispatch(&s.daemon, build_request(cmds.set_model, Some(data))).await;
    narrowed(resp, StageMutation::from_daemon)
}

/// `DELETE /pipeline/{stage}/model` — stop this stage, keeping its backend.
#[utoipa::path(
    delete,
    path = "/pipeline/{stage}/model",
    tag = "pipeline",
    summary = "Stop a stage's model",
    description = "\
Unloads the model, freeing its device memory, and leaves the backend selected so a \
different model can be loaded without re-selecting it. To empty the stage entirely, \
use `DELETE /pipeline/{stage}`.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Unloaded.", body = StageMutation),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn clear_stage_model(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    let resp = dispatch(&s.daemon, build_request(cmds.clear_model, None)).await;
    narrowed(resp, StageMutation::from_daemon)
}

/// `POST /pipeline/{stage}/model/cancel` — abandon the load this stage has in
/// flight. Scoped to the stage: the stages provision independently, so one
/// stage's cancel is not a licence to abandon another's download.
#[utoipa::path(
    post,
    path = "/pipeline/{stage}/model/cancel",
    tag = "pipeline",
    summary = "Abandon a stage's in-flight load",
    description = "\
Stops the download or load this stage has in flight. Scoped to the stage: the stages \
provision independently, so cancelling one is not a licence to abandon another's \
download.

`409 no_switch_in_progress` when the stage has nothing in flight.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Cancelled.", body = crate::daemon::http::wire::Ack),
        (status = 409, description = "Nothing was in flight for this stage (`no_switch_in_progress`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn cancel_stage_model(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    let resp = dispatch(&s.daemon, build_request(cmds.cancel_model, None)).await;
    narrowed(resp, crate::daemon::http::wire::Ack::from_daemon)
}

/// `POST /pipeline/{stage}/model/reload` — re-instantiate in place, picking up
/// changed secrets and options without a manual unload/load.
#[utoipa::path(
    post,
    path = "/pipeline/{stage}/model/reload",
    tag = "pipeline",
    summary = "Re-instantiate a stage's model in place",
    description = "\
Tears the model down and brings it back up so it picks up changed secrets and \
options — an API key set through `/backends/{backend_id}/secrets`, say — without the \
client having to unload and reload by hand.

Nothing is re-downloaded; the files on disk are unchanged.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Reloaded.", body = crate::daemon::http::wire::Ack),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn reload_stage_model(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    let resp = dispatch(&s.daemon, build_request(cmds.reload_model, None)).await;
    narrowed(resp, crate::daemon::http::wire::Ack::from_daemon)
}

/// `GET /pipeline/{stage}/model/list` — the models this stage can run.
#[utoipa::path(
    get,
    path = "/pipeline/{stage}/model/list",
    tag = "pipeline",
    summary = "List the models a stage can run",
    description = "\
The models the backend filling this stage serves *in this stage's role* — \
transcription models for stage 1, post-processors for stage 2. Fill a model picker \
from this.

Scoped twice over, and both halves matter. A model from another backend cannot load \
here; a model with the wrong role loads and then fails on every use, which for a \
post-processor picked as a transcription model means each recording fails after the \
user has already spoken.

The full catalog, every installed backend and every role, is `GET /backends`. This is \
the narrow read a stage's picker wants, and it is answered per stage precisely so a \
client does not have to re-derive roles for itself.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The models on offer, `(name, source)` pairs. Empty when the stage has no backend.", body = ModelList),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn list_stage_models(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    let resp = dispatch(&s.daemon, build_request(cmds.list_models, None)).await;
    narrowed(resp, ModelList::from_daemon)
}
