// SPDX-License-Identifier: GPL-3.0-only
//! Model selection and loading.
//!
//! The daemon is a backend orchestrator: it discovers backends on disk
//! ([`crate::stt_models::backends`]) and routes `(name, source)` to
//! the backend that serves the model, instantiating it as a `dyn Transcribe`.
//! There is no in-tree inference and no model download path here — subprocess
//! backends provision their own files when spawned.
//!
//! Every load and unload of every stage is the engine's
//! ([`super_engine_daemon::stage`]); [`stage`] says what each stage is.

mod discovery;
pub(crate) mod idle;
mod instantiate;
mod lifecycle;
mod post_processor;
pub(crate) mod stage;
mod switch;

use super_engine_daemon::stage::Load;
use super_voice_shared::models::protocol::DaemonResponse;

/// The answer to a request whose load gave up for a newer one, which loads
/// what the user asked for last.
pub(in crate::daemon) fn superseded(action: &str) -> DaemonResponse {
    let msg = format!("Model {action} failed: a newer model load replaced it");
    log::warn!("{msg}");
    DaemonResponse::error(&msg)
}

/// The answer to a switch or a reload of a stage's model (`action` names
/// which), by how its load ended. Nothing to load is the caller's to answer.
pub(in crate::daemon) fn load_response(action: &str, load: Load) -> DaemonResponse {
    match load {
        Load::Loaded { model, .. } => DaemonResponse::success()
            .with_current_model(model.name.clone())
            .with_current_source(model.source)
            .with_message(format!("Successfully switched to model: {}", model.name)),
        Load::Unchanged => DaemonResponse::success(),
        Load::Superseded => superseded(action),
        Load::Cancelled => {
            DaemonResponse::error(&format!("Model {action} cancelled due to shutdown"))
        }
        Load::Failed { error, .. } => {
            DaemonResponse::error(&format!("Model {action} failed: {error:#}"))
        }
    }
}
