// SPDX-License-Identifier: GPL-3.0-only
//! Model selection and loading.
//!
//! The daemon is a backend orchestrator: it discovers backends on disk
//! ([`crate::tts::tts_models::backends`]) and routes `(name, source)` to
//! the backend that serves the model, instantiating it as a `dyn Synthesize`.
//! There is no in-tree inference and no model download path here — subprocess
//! backends provision their own files when spawned.
//!
//! Every load and unload is the engine's ([`super_engine_daemon::stage`]), the
//! same as for speech to text's stages; [`stage`] says what synthesis is.

mod discovery;
mod idle;
mod instantiate;
mod lifecycle;
pub(crate) mod stage;
mod switch;

use super_engine_daemon::stage::Load;
use super_voice_shared::tts::models::protocol::DaemonResponse;

/// The answer to a request whose load gave up for a newer one, which loads
/// what the user asked for last.
pub(in crate::tts::daemon) fn superseded(action: &str) -> DaemonResponse {
    let msg = format!("Model {action} failed: a newer model load replaced it");
    log::warn!("{msg}");
    DaemonResponse::error(&msg)
}

/// The answer to a switch or a reload of the synthesis model (`action` names
/// which), by how its load ended. Nothing to load is the caller's to answer.
pub(in crate::tts::daemon) fn load_response(action: &str, load: Load) -> DaemonResponse {
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
