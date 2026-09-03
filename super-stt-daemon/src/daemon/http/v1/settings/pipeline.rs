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

use crate::daemon::http::internal::helpers::dispatch::{
    build_request, dispatch, dispatch_command, json_response,
};
use crate::daemon::http::state::AppState;
use crate::daemon::http::v1::backends::json_error_msg;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use super_stt_shared::models::protocol::DaemonResponse;

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
pub(crate) async fn get_pipeline(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "get_pipeline", None).await
}

/// `GET /pipeline/{stage}` — one stage, from the same report.
pub(crate) async fn get_stage(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> impl IntoResponse {
    if Stage::resolve(stage).is_none() {
        return unknown_stage(stage);
    }
    let resp = dispatch(&s.daemon, build_request("get_pipeline", None)).await;
    // Narrow the array to the requested position rather than re-deriving it, so
    // one stage and the whole list can never disagree.
    let one = resp.pipeline.as_ref().and_then(|stages| {
        stages
            .as_array()?
            .iter()
            .find(|v| v.get("stage").and_then(serde_json::Value::as_u64) == Some(u64::from(stage)))
            .cloned()
    });
    match one {
        Some(v) => json_response(&DaemonResponse::success().with_stage(v)).into_response(),
        None => unknown_stage(stage),
    }
}

#[derive(Deserialize)]
pub(crate) struct SetBackendBody {
    pub(crate) source: String,
}

/// `POST /pipeline/{stage}` — select the backend filling this stage.
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
    json_response(&resp).into_response()
}

/// `DELETE /pipeline/{stage}` — deselect this stage's backend.
pub(crate) async fn clear_stage_backend(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    dispatch_command(&s.daemon, cmds.clear_backend, None)
        .await
        .into_response()
}

#[derive(Deserialize)]
pub(crate) struct SetModelBody {
    pub(crate) model: String,
    /// Omitted resolves to the backend selected for this stage.
    #[serde(default)]
    pub(crate) source: Option<String>,
}

/// `POST /pipeline/{stage}/model` — run a model in this stage.
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
    json_response(&resp).into_response()
}

/// `DELETE /pipeline/{stage}/model` — stop this stage, keeping its backend.
pub(crate) async fn clear_stage_model(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    dispatch_command(&s.daemon, cmds.clear_model, None)
        .await
        .into_response()
}

/// `POST /pipeline/{stage}/model/cancel` — abandon the load this stage has in
/// flight. Scoped to the stage: the stages provision independently, so one
/// stage's cancel is not a licence to abandon another's download.
pub(crate) async fn cancel_stage_model(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    dispatch_command(&s.daemon, cmds.cancel_model, None)
        .await
        .into_response()
}

/// `POST /pipeline/{stage}/model/reload` — re-instantiate in place, picking up
/// changed secrets and options without a manual unload/load.
pub(crate) async fn reload_stage_model(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    dispatch_command(&s.daemon, cmds.reload_model, None)
        .await
        .into_response()
}

/// `GET /pipeline/{stage}/model/{model}/device` — the device `model` prefers,
/// what it resolved to, and what this install can offer it.
pub(crate) async fn get_model_device(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    dispatch_command(
        &s.daemon,
        cmds.get_model_device,
        Some(serde_json::json!({ "model": model })),
    )
    .await
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct SetDeviceBody {
    pub(crate) device: String,
}

/// `POST /pipeline/{stage}/model/{model}/device` — run `model` on `device`,
/// reloading it when it is the one this stage is running.
pub(crate) async fn set_model_device(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
    axum::Json(body): axum::Json<SetDeviceBody>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    dispatch_command(
        &s.daemon,
        cmds.set_model_device,
        Some(serde_json::json!({ "model": model, "device": body.device })),
    )
    .await
    .into_response()
}

/// `GET /pipeline/{stage}/model/{model}/device/list` — the devices this
/// install can offer `model` on this host.
pub(crate) async fn list_model_devices(
    State(s): State<AppState>,
    Path((stage, model)): Path<(u32, String)>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    dispatch_command(
        &s.daemon,
        cmds.list_model_devices,
        Some(serde_json::json!({ "model": model })),
    )
    .await
    .into_response()
}

/// `GET /pipeline/{stage}/device/list` — the devices the backend selected for
/// this stage can be run on here.
pub(crate) async fn list_stage_devices(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    let Some(cmds) = Stage::resolve(stage) else {
        return unknown_stage(stage);
    };
    dispatch_command(&s.daemon, cmds.list_backend_devices, None)
        .await
        .into_response()
}
