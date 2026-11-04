// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}` — one stage's backend: read it, fill it, empty it.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline/stage.md`.
//!
//! Only the backend. What it is running is `/pipeline/{stage}/model`, next door
//! in [`super::model`]. The pair is deliberate — a card's Select and its Load
//! are separate acts, and Deselect here forgets the backend where Unload there
//! keeps it, so a user who only wanted their VRAM back does not lose the choice
//! they made.

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Deserialize;
use super_tts_shared::models::protocol::DaemonResponse;

use super::{Stage, unknown_stage};
use crate::daemon::http::v1::wire::{FromDaemon, PipelineReport, StageEnvelope, StageMutation};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};

/// `GET /pipeline/{stage}` — one stage, from the same report.
#[utoipa::path(
    get,
    path = "/pipeline/{stage}",
    tag = "pipeline",
    summary = "Report one pipeline stage",
    description = "\
The backend filling this position, and whether the stage is switched on. The same \
object `GET /pipeline` carries in its array, for a client that only cares about one \
position — narrowed from that report rather than derived separately, so one stage and \
the whole list can never disagree.

An empty stage answers with `source` and `name` as `null` and `enabled` false rather \
than omitting the keys, so a card can read `source` to decide whether the stage is \
filled without first checking that the key exists.

The model is not here: read it at `GET /pipeline/{stage}/model`. A card showing \
\"Kokoro, kokoro-82m, on the GPU\" is two reads, and they are separate because the \
model read can be answering mid-download while the backend selection has been settled \
since the user clicked.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The stage.", body = StageEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn get_stage(State(s): State<AppState>, Path(stage): Path<u32>) -> Response {
    if Stage::resolve(stage).is_none() {
        return unknown_stage(stage);
    }
    let resp = dispatch(&s.daemon, build_request("get_pipeline", None)).await;
    // A failed read is the daemon's failure, not a missing position. Answering
    // it as `404 unknown_stage` would tell a client the stage it asked for does
    // not exist, and a card that hid itself on that answer would stay hidden
    // long after the daemon recovered. `narrowed` passes the daemon's own
    // envelope — status code, `error_code` and all — straight through.
    if resp.status != "success" {
        return narrowed(resp, PipelineReport::from_daemon);
    }
    // Narrow the array to the requested position rather than re-deriving it, so
    // one stage and the whole list can never disagree.
    let one = resp
        .pipeline
        .as_ref()
        .and_then(|stages| stages.iter().find(|st| st.stage == stage).cloned());
    match one {
        Some(stage) => narrowed(DaemonResponse::success(), |_| StageEnvelope {
            status: "success",
            stage,
        }),
        // A daemon that reported a pipeline without this position does not have
        // it, whatever the resolver above believes — which is the same answer,
        // reached from the other side.
        None => unknown_stage(stage),
    }
}

/// Which backend should fill the stage.
#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct SetBackendBody {
    /// The backend's repo id, as `GET /pipeline/{stage}/backend/list` reports
    /// it.
    #[schema(example = "github.com/jorge-menjivar/super-tts-kokoro")]
    pub(crate) source: String,
}

/// `POST /pipeline/{stage}` — select the backend filling this stage.
#[utoipa::path(
    post,
    path = "/pipeline/{stage}",
    tag = "pipeline",
    summary = "Select the backend filling a stage",
    description = "\
Points a stage at an installed backend. Validates that its files are on disk and that \
it serves this stage's role; it does not load anything, so it cannot fail for the \
runtime reasons a load can. Run a model with `POST /pipeline/{stage}/model` afterwards.

Fill the picker behind this from `GET /pipeline/{stage}/backend/list` rather than from \
`GET /backend/list`: a backend serving nothing this stage can run is refused here, and \
offering one hands the user an error to discover by choosing it.

Selecting a *different* backend drops the model with it — a model name belongs to the \
backend that served it, and two backends serving the same name do not serve the same \
model — and switches the stage off until a model is chosen, stopping anything that \
model was saying. Re-selecting the backend already there changes nothing.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
    ),
    request_body = SetBackendBody,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Selected. `active_backend` is what the stage now holds.", body = StageMutation),
        (status = 400, description = "`source` was missing or empty (`invalid_value`), or no installed backend has that `source` and serves this stage's role (`invalid_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn set_stage_backend(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
    axum::Json(body): axum::Json<SetBackendBody>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    let resp = dispatch(
        &s.daemon,
        build_request(
            cmds.set_backend,
            Some(serde_json::json!({ "source": body.source })),
        ),
    )
    .await;
    narrowed(resp, StageMutation::from_daemon)
}

/// `DELETE /pipeline/{stage}` — deselect this stage's backend.
#[utoipa::path(
    delete,
    path = "/pipeline/{stage}",
    tag = "pipeline",
    summary = "Empty a stage",
    description = "\
Deselects the stage's backend, unloading its model first if one is up, and forgets \
both. Anything being spoken is stopped. Emptying stage 1 returns the daemon to idle: \
`POST /speak` answers `409 model_not_loaded` until a backend and a model are chosen \
again.

This is not what a Stop button should call. Freeing the device memory while keeping \
the choice the user made is `DELETE /pipeline/{stage}/model`, which leaves the backend \
selected so the same model — or another of its models — can be loaded again without \
picking it a second time.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Emptied. `active_backend` is an explicit `null`.", body = StageMutation),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn clear_stage_backend(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    let resp = dispatch(&s.daemon, build_request(cmds.clear_backend, None)).await;
    narrowed(resp, StageMutation::from_daemon)
}
