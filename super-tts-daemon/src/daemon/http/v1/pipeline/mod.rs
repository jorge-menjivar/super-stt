// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline` — the ordered stages an utterance passes through.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline.md`.
//!
//! The modules mirror the paths: [`stage`] serves `/pipeline/{stage}`,
//! [`backend`] the menu that fills it, [`model`] serves
//! `/pipeline/{stage}/model` and its verbs, [`device`] and [`language`] serve
//! the two per-model preferences. `GET /pipeline`, the whole report, is here at
//! the root because that is where the path is.
//!
//! Super TTS has exactly one stage — 1, synthesis — and addresses it by
//! position anyway. That is the whole design: a stage inserted later (a text
//! normalizer that expands abbreviations before the voice ever sees them) is a
//! new *row* in [`Stage::resolve`] rather than a new endpoint family, and a
//! client written against `/pipeline/1` already knows how to drive it. Every
//! stage answers the same verbs — select a backend, deselect it, run a model,
//! stop it, and read or set the device and language one of its models uses —
//! so a client learns one shape and applies it at any position.
//!
//! What differs per stage is only *which* daemon command implements the verb,
//! which is what [`Stage`] resolves. The handlers are therefore
//! position-independent: there is one implementation of each operation, and a
//! fix to it cannot land at one position and not another.

pub(crate) mod backend;
pub(crate) mod device;
pub(crate) mod language;
pub(crate) mod model;
pub(crate) mod stage;
pub(crate) mod voice;

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

/// The commands that implement one stage's verbs.
///
/// A table rather than a `match` inside each handler: the handlers name a verb
/// (`cmds.set_model`) and never a position, so adding a stage is one more arm
/// in [`Stage::resolve`] and no change at all to the twelve paths above it.
/// Spelling the command inline in each handler is what would let a new stage
/// pick up eleven of a stage's twelve verbs and quietly keep the twelfth
/// pointed at stage 1's.
struct Stage {
    /// The installed backends this stage can be filled with.
    list_backends: &'static str,
    /// Select this stage's backend.
    set_backend: &'static str,
    /// Deselect it.
    clear_backend: &'static str,
    /// Read this stage's model slot.
    get_model: &'static str,
    /// Run a model in this stage.
    set_model: &'static str,
    /// Stop it, keeping the backend selected.
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
    ///
    /// One row today. The commands it names are the ones the daemon has always
    /// had — synthesis was "the active backend" and "the active model" before
    /// it was a position — so the rename happened at the URL and not on the
    /// command bus.
    fn resolve(stage: u32) -> Option<Self> {
        match stage {
            super_tts_shared::models::protocol::SYNTHESIS_STAGE => Some(Self {
                list_backends: "list_backends",
                set_backend: "set_active_backend",
                clear_backend: "clear_active_backend",
                get_model: "get_model",
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
            _ => None,
        }
    }
}

/// `404 unknown_stage`, naming the positions that do exist.
///
/// The message carries the list because the number in the URL is the client's
/// only handle on the pipeline's shape: a client asking for stage 2 is asking
/// about a build it cannot see the shape of, and "no stage 2" alone leaves it
/// unable to tell a not-yet-shipped position from a typo. Told what exists, it
/// can surface "this build has one stage" rather than a transport failure.
fn unknown_stage(stage: u32) -> Response {
    json_error_msg(
        StatusCode::NOT_FOUND,
        "unknown_stage",
        &format!("No stage {stage} in the pipeline. The only stage is 1 (synthesis)."),
    )
}

/// `GET /pipeline` — every stage, in order.
#[utoipa::path(
    get,
    path = "/pipeline",
    tag = "pipeline",
    summary = "Report every pipeline stage",
    description = "\
The ordered stages an utterance passes through on its way from text to audio. Stage 1 \
is synthesis, and today it is the only one — the array is how a stage added ahead of \
it would arrive.

Each stage reports the backend filling it and whether the stage is switched on. What \
that backend is *running* is one level down, at `GET /pipeline/{stage}/model` — a \
stage is a durable selection that cannot fail for runtime reasons, while its model \
downloads, allocates and can. Reporting the two together is what lets a client \
confuse \"no backend chosen\" with \"chosen, but its model is not up\".

Read one position on its own with `GET /pipeline/{stage}`; it is narrowed from this \
same report, so the two can never disagree.",
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
        .routes(routes!(
            model::get_stage_model,
            model::set_stage_model,
            model::clear_stage_model
        ))
        .routes(routes!(model::list_stage_models))
        .routes(routes!(model::cancel_stage_model))
        .routes(routes!(model::reload_stage_model))
        .routes(routes!(backend::list_stage_backends))
        .routes(routes!(device::list_stage_devices))
        .routes(routes!(device::get_model_device, device::set_model_device))
        .routes(routes!(device::list_model_devices))
        .merge(language::routes())
        .merge(voice::routes())
}
