// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}` — one stage's backend: read it, fill it, empty it.
//!
//! Only the backend. What it is running is `/pipeline/{stage}/model`, next door
//! in [`super::model`]. The pair is deliberate — a card's Select and its Load
//! are separate acts, and Deselect here forgets the backend where Unload there
//! keeps it.

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Deserialize;
use super_stt_shared::models::protocol::DaemonResponse;

use super::{Stage, unknown_stage};
use crate::daemon::http::v1::wire::{FromDaemon, StageEnvelope, StageMutation};
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

The model is not here: read it at `GET /pipeline/{stage}/model`.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
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
        None => unknown_stage(stage),
    }
}

/// Which backend should fill the stage.
#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct SetBackendBody {
    /// The backend's repo id, as `GET /backends` reports it.
    #[schema(example = "github.com/acme/whisper")]
    pub(crate) source: String,
}

/// `POST /pipeline/{stage}` — select the backend filling this stage.
#[utoipa::path(
    post,
    path = "/pipeline/{stage}",
    tag = "pipeline",
    summary = "Select the backend filling a stage",
    description = "\
Points a stage at an installed backend. Selecting a backend does not load a model — \
do that with `POST /pipeline/{stage}/model`.

A backend that serves nothing this stage can run is refused: filling stage 2 with a \
transcription-only backend would leave the user staring at an empty model picker \
with no reason given.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
    ),
    request_body = SetBackendBody,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Selected.", body = StageMutation),
        (status = 400, description = "No installed backend has that `source`, or it serves nothing this stage can run (`invalid_backend`).", body = ErrorEnvelope),
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
Deselects the stage's backend, unloading its model first if one is up. The stage \
then does nothing: stage 1 empty means no transcription, stage 2 empty means \
transcripts pass through unrewritten.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Emptied.", body = StageMutation),
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
