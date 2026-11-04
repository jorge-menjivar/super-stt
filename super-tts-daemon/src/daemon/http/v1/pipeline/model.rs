// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model` — the model a stage is pointed at.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline/model.md`.
//!
//! Read it, run it, stop it, abandon a load still in flight, or re-instantiate
//! it in place. All of them are scoped to the stage in the path: stages
//! provision independently, so one stage's cancel must not abandon another's
//! download.
//!
//! The read is where the selection lives. `GET /pipeline/{stage}` reports the
//! *backend* filling the position and nothing else; what that backend is
//! running — and whether it is running at all — is here. This is also the read
//! `POST /speak` depends on: an utterance needs a stage whose model is
//! `loaded`.

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Deserialize;

use super::{Stage, unknown_stage};
use crate::daemon::http::v1::wire::{FromDaemon, ModelList, StageModelEnvelope, StageMutation};
use crate::daemon::http::wire::{Ack, ErrorEnvelope, ReasonEnvelope};

/// `GET /pipeline/{stage}/model` — the model this stage is pointed at.
#[utoipa::path(
    get,
    path = "/pipeline/{stage}/model",
    tag = "pipeline",
    summary = "Report a stage's model",
    description = "\
What the stage is pointed at, whether it is loaded, which accelerator it runs on, and \
the load still in flight — in one payload, so a settings card renders its whole Model \
section from a single request.

`model` is the *selection*, not the running instance: it survives an unload, so a card \
can offer to load the same model again — onto another device, say — without the user \
picking it a second time. `loaded` is what says whether it is up, and what `POST \
/speak` requires. `switch` can be present while `model` is still `null`; that is a \
stage's very first load.

`device` carries the stored preference and what it resolved to, which is all a picker \
needs to render its current value. What the model *could* run on is \
`GET /pipeline/{stage}/model/{model}/device/list`, kept separate because that answer \
costs a fresh probe of the host's accelerators.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The stage's model slot. `model` is `null` when nothing is selected — an empty slot, not an absent key.", body = StageModelEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn get_stage_model(State(s): State<AppState>, Path(stage): Path<u32>) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    let mut resp = dispatch(&s.daemon, build_request(cmds.get_model, None)).await;
    // An idle stage is not a failure, and the command still says it is: with
    // nothing loaded it answers "No model is currently loaded" as an *error*
    // envelope, while attaching the slot anyway. The slot is the answer — a
    // stage with nothing selected has a slot with nothing in it — so a response
    // carrying one is a success however it labelled itself.
    //
    // The endpoint this replaced compensated the same way. Without it the very
    // first read on a fresh install is a `500`, and a Model card can never
    // render the empty state it is supposed to open in.
    if resp.stage_model.is_some() && resp.status != "success" {
        resp.status = "success".to_string();
        resp.error_code = None;
    }
    narrowed(resp, StageModelEnvelope::from_daemon)
}

/// Which model to run in the stage.
#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct SetModelBody {
    /// The model's name, as `GET /pipeline/{stage}/model/list` spells it.
    #[schema(example = "kokoro-82m")]
    pub(crate) model: String,
    /// The backend serving it. Omitted resolves to the backend already selected
    /// for this stage.
    #[serde(default)]
    #[schema(example = "github.com/jorge-menjivar/super-tts-kokoro")]
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
yet. That download can be long, so the switch runs on: watch the `download_progress` \
and `daemon_status_changed` topics on `GET /events`, or poll \
`GET /pipeline/{stage}/model` and read `switch`. Abandon it with \
`POST /pipeline/{stage}/model/cancel`.

Omitting `source` uses the backend already selected for the stage; naming one also \
selects that backend, so a client can go straight from a flat model picker to a load. \
Two backends may serve the same model name, which is why an omitted `source` resolves \
against the stage's selection rather than by scanning for whoever serves the name.

A load that fails leaves the backend selected and *no* model loaded. The daemon does \
not quietly restore whatever was running before, because a client that asked for a \
switch and was told it succeeded would then be speaking in the old voice.

Anything being spoken is stopped first: the switch is the newer request, the same way \
a second `POST /speak` supersedes the first. Asking for the model that is already \
running changes nothing and interrupts nothing.

The model loads on its own device — set that first with \
`POST /pipeline/{stage}/model/{model}/device`, which is also how to move a model that \
is already running.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
    ),
    request_body = SetModelBody,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Accepted. The model may still be downloading — read `switch` on `GET /pipeline/{stage}/model` to follow it.", body = StageMutation),
        (status = 400, description = "`model` was missing or empty (`invalid_value`); no installed backend serves `(model, source)` in this stage's role (`invalid_model`); `source` was omitted and the stage has no backend (`invalid_backend`); or the model is online and `allow_online_models` is off (`online_models_disabled`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 409, description = "This stage already has a load in flight (`switch_in_progress`).", body = ErrorEnvelope),
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
    // `source` is added only when the client sent one: the key's *absence* is
    // what tells the daemon to resolve against the stage's own backend, where a
    // `null` would read as a request to load from no backend at all.
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
Unloads the model, freeing its device memory, and leaves the backend selected so \
another of its models — or the same one, on another device — can be loaded without \
re-selecting it.

This is what a Stop button should call, and it stops whatever is being spoken first — \
a Stop that left the audio playing would not be one. Emptying the stage entirely, \
forgetting the backend along with the model, is `DELETE /pipeline/{stage}`.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Unloaded. The backend is still selected.", body = StageMutation),
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
/// flight. Scoped to the stage: stages provision independently, so one stage's
/// cancel is not a licence to abandon another's download.
#[utoipa::path(
    post,
    path = "/pipeline/{stage}/model/cancel",
    tag = "pipeline",
    summary = "Abandon a stage's in-flight load",
    description = "\
Stops the download or load this stage has in flight, leaving the stage with whatever \
it had before the switch started. Scoped to the stage: stages provision independently, \
so cancelling one is not a licence to abandon another's download.

`409 no_switch_in_progress` when the stage has nothing in flight, and \
`409 switch_finalizing` once the switch has passed the point where stopping it would \
leave a half-written model on disk.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Cancelled.", body = Ack),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 409, description = "Nothing was in flight for this stage (`no_switch_in_progress`), or it is past the cancellable window (`switch_finalizing`).", body = ErrorEnvelope),
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
    narrowed(resp, Ack::from_daemon)
}

/// `POST /pipeline/{stage}/model/reload` — re-instantiate in place, picking up
/// changed secrets and options without a manual unload/load.
#[utoipa::path(
    post,
    path = "/pipeline/{stage}/model/reload",
    tag = "pipeline",
    summary = "Re-instantiate a stage's model in place",
    description = "\
Tears the model down and brings it back up as the same `(model, source)` on the same \
device, so it picks up a changed secret or option — an API key written through \
`/backend/{backend_id}/secret/{name}`, say — without the client unloading and \
reloading by hand.

Synchronous, unlike `POST /pipeline/{stage}/model`: the response is sent once the \
model is back up, and the daemon then broadcasts the same `model_switched` and `ready` \
events a completed switch does, so subscribers converge on one state either way. \
Nothing is re-downloaded; the files on disk are unchanged, and a stage with nothing \
loaded answers `200` having done nothing. Anything the model is saying is stopped \
first, since the instance saying it is the one being torn down.

Rarely needed by hand — writing a backend option or secret already reloads every stage \
running a model from that backend.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Reloaded, or nothing was loaded to reload; `message` says which.", body = Ack),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 500, description = "Re-instantiation failed; the previous instance is gone and the stage is left with nothing loaded.", body = ErrorEnvelope),
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
    narrowed(resp, Ack::from_daemon)
}

/// `GET /pipeline/{stage}/model/list` — the models this stage can run.
#[utoipa::path(
    get,
    path = "/pipeline/{stage}/model/list",
    tag = "pipeline",
    summary = "List the models a stage can run",
    description = "\
The models the backend filling this stage serves *in this stage's role* — synthesis \
models for stage 1 — as `[name, source]` pairs, which is the pair \
`POST /pipeline/{stage}/model` accepts. Fill a model picker from this.

Scoped twice over, and both halves matter. A model from another backend cannot load \
here without also changing the stage's backend, so offering it hands the user a pick \
that does something other than what the list implied. The role filter is invisible \
today, since every model a TTS backend serves synthesizes; it stops being invisible \
the moment a second position exists, and doing it in the daemon rather than in each \
client is what keeps the offer and the acceptance from drifting.

The full catalog — every installed backend, with the voices, language tags and device \
support of each — is `GET /backend/list`. This is the narrow read a stage's picker \
wants.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The models on offer, `[name, source]` pairs. Empty when the stage has no backend selected, which reads correctly as \"choose a backend first\".", body = ModelList),
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
