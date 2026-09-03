// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline`, `/pipeline/{stage}` and `/pipeline/{stage}/model` — the ordered
//! stages a transcript passes through.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline.md`.
//!
//! Every stage answers the same verbs — select a backend, deselect it, run a
//! model, stop it, and read or set the device one of its models runs on — so a
//! client learns one shape and applies it at any position. What differs per
//! stage is only *which* command implements the verb, which is what [`Stage`]
//! resolves; the handlers themselves are the ones each stage always had, so
//! there is a single implementation of each operation.

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use crate::daemon::http::v1::backends::json_error_msg;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use super_stt_shared::models::protocol::DaemonResponse;

use super::wire::{
    DeviceList, FromDaemon, ModelDevice, PipelineReport, StageEnvelope, StageMutation,
};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};

/// The commands that implement one stage's four verbs.
///
/// Adding a third stage means one more arm here — not a new endpoint — which is
/// the whole point of addressing stages by position.
struct Stage {
    /// Select this stage's backend.
    set_backend: &'static str,
    /// Deselect it.
    clear_backend: &'static str,
    /// Run a model in this stage.
    set_model: &'static str,
    /// Stop it.
    clear_model: &'static str,
    /// Abandon the load this stage has in flight.
    cancel_model: &'static str,
    /// Re-instantiate in place to pick up changed secrets/options.
    reload_model: &'static str,
    /// Read the device one of this stage's models runs on.
    get_model_device: &'static str,
    /// Set it.
    set_model_device: &'static str,
    /// The devices one of this stage's models can be run on here.
    list_model_devices: &'static str,
    /// The devices this stage's backend can be run on here.
    list_backend_devices: &'static str,
}

impl Stage {
    /// Resolve a stage number, or `None` when the pipeline has no such stage.
    fn resolve(stage: u32) -> Option<Self> {
        match stage {
            1 => Some(Self {
                set_backend: "set_active_backend",
                clear_backend: "clear_active_backend",
                set_model: "set_model",
                clear_model: "unload_active_model",
                cancel_model: "cancel_download",
                reload_model: "reload_active_model",
                get_model_device: "get_model_device",
                set_model_device: "set_model_device",
                list_model_devices: "list_model_devices",
                list_backend_devices: "list_active_backend_devices",
            }),
            2 => Some(Self {
                set_backend: "set_post_processor_backend",
                clear_backend: "clear_post_processor_backend",
                set_model: "set_post_processor",
                clear_model: "clear_post_processor",
                cancel_model: "cancel_post_processor_download",
                reload_model: "reload_post_processor",
                get_model_device: "get_post_processor_device",
                set_model_device: "set_post_processor_device",
                list_model_devices: "list_post_processor_devices",
                list_backend_devices: "list_post_processor_backend_devices",
            }),
            _ => None,
        }
    }
}

/// `404 unknown_stage`, naming the positions that do exist — a client asking for
/// stage 3 today is asking about a pipeline it cannot see the shape of.
fn unknown_stage(stage: u32) -> Response {
    json_error_msg(
        StatusCode::NOT_FOUND,
        "unknown_stage",
        &format!(
            "No stage {stage} in the pipeline. Stages are 1 (transcription) and 2 (post-processing)."
        ),
    )
}

/// `GET /pipeline` — every stage, in order.
#[utoipa::path(
    get,
    path = "/pipeline",
    tag = "pipeline",
    summary = "Report every pipeline stage",
    description = "\
The ordered stages a transcript passes through. Stage 1 turns audio into text; every \
later stage rewrites what the one before it produced.

Each stage reports which backend fills it, which model is selected, whether that \
model is up, the accelerator it actually runs on, and any load still in flight. \
Stages are addressed by position precisely so a third can be appended without \
inventing a third endpoint for it.",
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Every stage, stage 1 first.", body = PipelineReport),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn get_pipeline(State(s): State<AppState>) -> Response {
    let resp = dispatch(&s.daemon, build_request("get_pipeline", None)).await;
    narrowed(resp, PipelineReport::from_daemon)
}

/// `GET /pipeline/{stage}` — one stage, from the same report.
#[utoipa::path(
    get,
    path = "/pipeline/{stage}",
    tag = "pipeline",
    summary = "Report one pipeline stage",
    description = "\
The same object `GET /pipeline` carries in its array, for a client that only cares \
about one position. It is narrowed from that report rather than derived separately, \
so one stage and the whole list can never disagree.",
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
options — an API key set through `/backends/{source}/secrets`, say — without the \
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
        ("model" = String, Path, description = "The model's name, as `GET /models` or `GET /backends` spells it."),
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
        ("model" = String, Path, description = "The model's name, as `GET /models` or `GET /backends` spells it."),
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
        ("model" = String, Path, description = "The model's name, as `GET /models` or `GET /backends` spells it."),
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
