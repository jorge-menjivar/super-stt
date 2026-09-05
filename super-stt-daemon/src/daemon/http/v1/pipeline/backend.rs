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
//! builds its own list from `GET /backend/list` is reimplementing that rule — with
//! a picker that offers a backend the daemon then rejects as the failure mode.

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
`GET /backend/list` returns them.

Fill a stage's backend picker from this rather than from `GET /backend/list`: a backend \
serving nothing this stage can run is refused by `POST /pipeline/{stage}`, and offering \
one hands the user an error to discover by choosing it. A backend serving both roles \
appears in both stages' lists.

Empty when nothing installed serves this stage — the state that should read as \"install \
one\" rather than as an empty dropdown.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
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
