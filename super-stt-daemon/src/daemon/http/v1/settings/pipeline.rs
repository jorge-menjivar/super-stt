// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline`, `/pipeline/{stage}` and `/pipeline/{stage}/model` — the ordered
//! stages a transcript passes through.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline.md`.
//!
//! Every stage answers the same four verbs — select a backend, deselect it, run
//! a model, stop it — so a client learns one shape and applies it at any
//! position. What differs per stage is only *which* command implements the
//! verb, which is what [`Stage`] resolves; the handlers themselves are the ones
//! each stage always had, so there is a single implementation of each operation.

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
    /// Abandon an in-flight load, when the stage can be interrupted.
    cancel_model: Option<&'static str>,
    /// Re-instantiate in place to pick up changed secrets/options.
    reload_model: Option<&'static str>,
    /// Whether `set_model` takes the stage's `device` with it. Stage 1's device
    /// is a daemon-wide preference with its own endpoint (`/active_device`)
    /// and a reload of its own; a later stage's is a property of that stage.
    takes_device: bool,
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
                cancel_model: Some("cancel_download"),
                reload_model: Some("reload_active_model"),
                takes_device: false,
            }),
            2 => Some(Self {
                set_backend: "set_post_processor_backend",
                clear_backend: "clear_post_processor_backend",
                set_model: "set_post_processor",
                clear_model: "clear_post_processor",
                // A post-processor loads no weights it could be interrupted
                // mid-download for, and has no in-place reload yet: re-running
                // `POST .../model` does the same job.
                cancel_model: None,
                reload_model: None,
                takes_device: true,
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
    /// The stage's `cpu`/`gpu` preference. Omitted keeps the stored one.
    /// Stages whose device lives elsewhere refuse it (see [`Stage::takes_device`]).
    #[serde(default)]
    pub(crate) device: Option<String>,
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
    if let Some(device) = body.device {
        // Refused rather than ignored: a client that sent a device for stage
        // 1 expects the model to land on it, and silently loading it
        // elsewhere is the worse surprise.
        if !cmds.takes_device {
            return json_error_msg(
                StatusCode::BAD_REQUEST,
                "invalid_value",
                &format!(
                    "Stage {stage} does not take a device here; set it with POST /active_device."
                ),
            );
        }
        data["device"] = serde_json::Value::String(device);
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

/// `POST /pipeline/{stage}/model/cancel` — abandon an in-flight load.
pub(crate) async fn cancel_stage_model(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    stage_action(&s, stage, |c| c.cancel_model, "cancel").await
}

/// `POST /pipeline/{stage}/model/reload` — re-instantiate in place, picking up
/// changed secrets and options without a manual unload/load.
pub(crate) async fn reload_stage_model(
    State(s): State<AppState>,
    Path(stage): Path<u32>,
) -> Response {
    stage_action(&s, stage, |c| c.reload_model, "reload").await
}

/// Dispatch an optional per-stage action, distinguishing "no such stage" from
/// "this stage does not do that" — a client asking stage 2 to cancel a download
/// has a wrong model of the pipeline, not a typo in the path.
async fn stage_action(
    state: &AppState,
    position: u32,
    pick: fn(&Stage) -> Option<&'static str>,
    action: &str,
) -> Response {
    let Some(cmds) = Stage::resolve(position) else {
        return unknown_stage(position);
    };
    let Some(command) = pick(&cmds) else {
        return json_error_msg(
            StatusCode::NOT_FOUND,
            "unsupported_action",
            &format!("Stage {position} does not support {action}."),
        );
    };
    dispatch_command(&state.daemon, command, None)
        .await
        .into_response()
}
