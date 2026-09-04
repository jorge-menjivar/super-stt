// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline` — the ordered stages a transcript passes through.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline.md`.
//!
//! The modules mirror the paths: [`stage`] serves `/pipeline/{stage}`, [`model`]
//! serves `/pipeline/{stage}/model` and its verbs, [`device`] and [`language`]
//! serve the two per-model preferences. `GET /pipeline`, the whole
//! report, is here at the root because that is where the path is.
//!
//! Every stage answers the same verbs — select a backend, deselect it, run a
//! model, stop it, and read or set the device one of its models runs on — so a
//! client learns one shape and applies it at any position. What differs per
//! stage is only *which* command implements the verb, which is what [`Stage`]
//! resolves; the handlers themselves are the ones each stage always had, so
//! there is a single implementation of each operation.

pub(crate) mod device;
pub(crate) mod language;
pub(crate) mod model;
pub(crate) mod stage;

use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use crate::daemon::http::v1::backends::json_error_msg;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::daemon::http::v1::wire::{FromDaemon, PipelineReport};
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
    /// The models this stage can run: its backend's, carrying its role.
    list_models: &'static str,
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
                list_models: "list_models",
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
                list_models: "list_post_processor_models",
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

/// Every `/pipeline` route. Merged into the settings group, whose scope these
/// share.
///
/// No path appears here: `routes!` reads each one off the handler's
/// `#[utoipa::path]`, which is also what the `OpenAPI` document is generated
/// from. Handlers sharing a path are registered together, which is what makes
/// them one path item with several methods.
pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_pipeline))
        .routes(routes!(
            stage::get_stage,
            stage::set_stage_backend,
            stage::clear_stage_backend
        ))
        .routes(routes!(model::set_stage_model, model::clear_stage_model))
        .routes(routes!(model::list_stage_models))
        .routes(routes!(model::cancel_stage_model))
        .routes(routes!(model::reload_stage_model))
        .routes(routes!(device::list_stage_devices))
        .routes(routes!(device::get_model_device, device::set_model_device))
        .routes(routes!(device::list_model_devices))
        .merge(language::routes())
}
