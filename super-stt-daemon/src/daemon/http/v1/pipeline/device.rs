// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/device/list` and `/pipeline/{stage}/model/{model}/device`
//! — the accelerators a stage or one of its models can run on.
//!
//! The preference is per model, not per stage: two models in the same stage can
//! prefer different devices. The stage-level list is the broader answer, for a
//! client filling a picker before any model is chosen.

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
model. Use it before a model is chosen; once one is, the per-model list is the \
narrower and more accurate answer.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
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
The accelerator this model is set to run on, and — when the preference is the \
generic `gpu` — what it actually resolved to once loaded. The preference is per \
model, not per stage: two models in the same stage can prefer different devices.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` or `GET /backends` spells it."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The preference, and what it resolved to.", body = ModelDevice),
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
    /// An accelerator token from the stage's or model's device list — `cpu`,
    /// `cuda`, `vulkan`, or the generic `gpu`.
    #[schema(example = "cuda")]
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
Chooses the accelerator this model runs on. If it is the model the stage is \
currently running, it is reloaded onto the new device; otherwise the preference is \
stored and takes effect at the next load.

List what this host can offer with `GET /pipeline/{stage}/model/{model}/device/list`.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` or `GET /backends` spells it."),
    ),
    request_body = SetDeviceBody,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Preference set; this is the resulting report.", body = ModelDevice),
        (status = 400, description = "This host cannot offer that device (`invalid_device`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such stage (`unknown_stage`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
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
What this machine can actually offer this model — the intersection of the host's \
accelerators and the builds the model ships. Fill a device picker from this rather \
than from `GET /gpu_info`, which reports the hardware without regard to what the \
model supports.",
    params(
        ("stage" = u32, Path,
         description = "Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`.",
         example = 1),
        ("model" = String, Path, description = "The model's name, as `GET /pipeline/{stage}/model/list` or `GET /backends` spells it."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The devices on offer.", body = DeviceList),
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
