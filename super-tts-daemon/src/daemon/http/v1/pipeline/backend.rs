// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/backend/list` — the backends that can fill a stage.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline/backend-list.md`.
//!
//! The slot itself is `/pipeline/{stage}`, one level up: `GET` reports the
//! backend filling the position and `POST` chooses it. This is the menu that
//! `POST` will accept — the same relationship [`super::model`] has with
//! `/model/list`, and [`super::device`] with `/device/list`.
//!
//! It exists because the daemon already decides this. `POST /pipeline/{stage}`
//! refuses a backend that serves nothing the stage can run, and a client that
//! builds its own list from `GET /backend/list` is reimplementing that rule —
//! with a picker that offers a backend the daemon then rejects as the failure
//! mode. With `synthesis` the only role, the two lists happen to hold the same
//! backends today; that is a coincidence of having one stage, and a client
//! built on the general list would not notice the day it stops being true.

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use axum::extract::{Path, State};
use axum::response::Response;

use super::{Stage, unknown_stage};
use crate::daemon::http::v1::wire::{BackendCatalog, FromDaemon};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};

/// `GET /pipeline/{stage}/backend/list` — the installed backends this stage can
/// be filled with.
#[utoipa::path(
    get,
    path = "/pipeline/{stage}/backend/list",
    tag = "pipeline",
    summary = "List the backends that can fill a stage",
    description = "\
The installed backends serving at least one model this stage can run, in the shape \
`GET /backend/list` returns them — each with its models, voices, options and secrets.

Fill a stage's backend picker from this rather than from `GET /backend/list`: a \
backend serving nothing this stage can run is refused by `POST /pipeline/{stage}`, and \
offering one hands the user an error to discover by choosing it. The two lists hold \
the same backends while `synthesis` is the only role, so the choice costs nothing \
today and keeps a client correct once it is not.

Empty when nothing installed serves this stage — the state that should read as \
\"install a backend\" rather than as an empty dropdown. `GET /registry/backend/list` \
is where a client sends the user next.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The backends on offer.", body = BackendCatalog),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn list_stage_backends(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    let resp = dispatch(&s.daemon, build_request(cmds.list_backends, None)).await;
    narrowed(resp, BackendCatalog::from_daemon)
}
