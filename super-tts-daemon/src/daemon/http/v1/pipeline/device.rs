// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/device/list` and `/pipeline/{stage}/model/{model}/device`
//! — the accelerators a stage or one of its models can run on.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline/device.md`.
//!
//! The preference is per model, not per stage and not per daemon: a small
//! preset-voice model runs fine on the CPU while the cloning model beside it
//! needs the GPU, and the global `/active_device` this replaced overwrote one
//! choice with the other. It is stored against `(source, model)` and addressed
//! through the stage, because the stage is what resolves a bare model name to
//! the backend serving it — which is also why an empty stage is a
//! `400 invalid_backend` here.
//!
//! The stage-level list is the broader answer — the union over the models its
//! backend serves — for a client filling a picker before any model is chosen.
//! Showing a device control that will be wrong once a model is picked is worse
//! than showing the union and narrowing it.

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Deserialize;

use super::{Stage, unknown_stage};
use crate::daemon::http::v1::wire::{DeviceList, FromDaemon, ModelDevice};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};

/// `GET /pipeline/{stage}/device/list` — the devices the backend selected for
/// this stage can be run on here.
#[utoipa::path(
    get,
    path = "/pipeline/{stage}/device/list",
    tag = "pipeline",
    summary = "List the devices this stage's backend can run on",
    description = "\
The devices the backend filling this stage can run on this host, without naming a \
model: the union of the per-model lists over the models it serves in this stage's \
role, always in `cpu`, `gpu` order.

Use it before a model is chosen. Once one is, \
`GET /pipeline/{stage}/model/{model}/device/list` is the narrower and more accurate \
answer — a backend whose large model needs a GPU still offers `cpu` here because \
another of its models runs there.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The devices on offer.", body = DeviceList),
        (status = 400, description = "The stage has no backend selected (`invalid_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn list_stage_devices(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    let resp = dispatch(&s.daemon, build_request(cmds.list_backend_devices, None)).await;
    narrowed(resp, DeviceList::from_daemon)
}

/// `GET /pipeline/{stage}/model/{model}/device` — the device `model` prefers,
/// what it resolved to, and what this install can offer it.
#[utoipa::path(
    get,
    path = "/pipeline/{stage}/model/{model}/device",
    tag = "pipeline",
    summary = "Read a model's device preference",
    description = "\
The accelerator this model is set to run on, what that resolved to, and what this \
install can offer it — everything a device control needs for its current value.

`device` is the stored choice (`cpu`, `gpu`, or `none` for an online model that \
synthesizes remotely); `resolved_accel` is what actually loaded, so a `gpu` choice \
that fell back reads `cpu` here and a client is never told a device resolved before a \
load proved it. The preference is per model, not per stage: two models on one backend \
can live on different devices.

`model` is resolved against the backend filling this stage, the same resolution an \
omitted `source` gets on `POST /pipeline/{stage}/model`. It answers whether or not the \
model is loaded, which is how a card shows its device before Load.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` or `GET /backend/list` spells it. Resolved against the backend filling this stage."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The preference, what it resolved to, and what this host can offer it.", body = ModelDevice),
        (status = 400, description = "The stage has no backend selected, so there is nothing to resolve `model` against (`invalid_backend`), or that backend serves no such model (`invalid_model`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn get_model_device(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    // Only the model travels: the daemon resolves it against the backend
    // filling the stage itself, and answers `invalid_backend` when there is
    // none — the same resolution, and the same error, an omitted `source` gets
    // on `POST /pipeline/{stage}/model`.
    let req = build_request(
        cmds.get_model_device,
        Some(serde_json::json!({ "model": model })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, ModelDevice::from_daemon)
}

/// Which accelerator the model should run on.
#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct SetDeviceBody {
    /// `cpu` or `gpu`. `cuda` and `metal` are accepted input spellings and are
    /// normalized to `gpu`; anything else is refused.
    #[schema(example = "gpu")]
    pub(crate) device: String,
}

/// `POST /pipeline/{stage}/model/{model}/device` — run `model` on `device`,
/// reloading it when it is the one this stage is running.
#[utoipa::path(
    post,
    path = "/pipeline/{stage}/model/{model}/device",
    tag = "pipeline",
    summary = "Set a model's device preference",
    description = "\
Chooses the accelerator this model runs on. If it is the model the stage is currently \
running, it is reloaded onto the new device before the response is sent, stopping \
anything it was saying; otherwise the choice is recorded and the model's next load \
picks it up. Either way the choice is the \
model's own from then on, remembered per `(source, model)`.

A reload that fails puts the model back on the device it had, leaves the setting as it \
was, and answers `500` with the reason — a choice is not recorded against a load that \
did not happen. Asking for the device a loaded model is already on reloads nothing, \
but a `gpu` preference that fell back to the CPU *is* retried, which is what tracking \
the preference and the resolved accelerator separately is for.

Asking for `gpu` on a host that cannot provide one is not an error: the load falls \
back to the CPU and says so in `resolved_accel`, because slower speech is better than \
no speech. A client that wants to offer only what will work narrows its picker to \
`GET /pipeline/{stage}/model/{model}/device/list`.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` or `GET /backend/list` spells it. Resolved against the backend filling this stage."),
    ),
    request_body = SetDeviceBody,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Recorded, or reloaded; this is the resulting report, and `message` says which of the two happened.", body = ModelDevice),
        (status = 400, description = "`device` is not one this model's manifest declares, or it runs remotely and has no local device (`invalid_device`); the stage has no backend selected (`invalid_backend`); or that backend serves no such model (`invalid_model`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 500, description = "The reload failed; the model was put back on the device it had and the setting is unchanged.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn set_model_device(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
    axum::Json(body): axum::Json<SetDeviceBody>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    let req = build_request(
        cmds.set_model_device,
        Some(serde_json::json!({ "model": model, "device": body.device })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, ModelDevice::from_daemon)
}

/// `GET /pipeline/{stage}/model/{model}/device/list` — the devices this
/// install can offer `model` on this host.
#[utoipa::path(
    get,
    path = "/pipeline/{stage}/model/{model}/device/list",
    tag = "pipeline",
    summary = "List the devices a model can run on here",
    description = "\
What this machine can actually offer this model: the devices its manifest declares, \
narrowed to the accelerators its installed asset ships and to what the host has. A \
CUDA-only backend on a machine with no NVIDIA GPU installed its CPU asset, and is not \
offered a GPU here.

Fill a device picker from this rather than from `GET /gpu_info`, which reports the \
hardware without regard to what a model supports — that is the memory figure a large \
model's requirements are weighed against, not the source of a picker.

Empty for a model that runs remotely, and for a local model this install cannot run on \
any device. A client hides the device control on an empty list rather than \
special-casing a status.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position. `1` synthesizes — text in, audio out — and is the only position this build has; any other is a `404 unknown_stage` naming the ones that do exist.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` or `GET /backend/list` spells it. Resolved against the backend filling this stage."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The devices on offer.", body = DeviceList),
        (status = 400, description = "The stage has no backend selected, so there is nothing to resolve `model` against (`invalid_backend`), or that backend serves no such model (`invalid_model`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn list_model_devices(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    let req = build_request(
        cmds.list_model_devices,
        Some(serde_json::json!({ "model": model })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, DeviceList::from_daemon)
}
