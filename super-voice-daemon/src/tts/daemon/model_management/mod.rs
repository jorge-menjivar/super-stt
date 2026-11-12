// SPDX-License-Identifier: GPL-3.0-only
//! Model selection and loading.
//!
//! The daemon is a backend orchestrator: it discovers backends on disk
//! ([`crate::tts::tts_models::backends`]) and routes `(name, source)` to
//! the backend that serves the model, instantiating it as a `dyn Synthesize`.
//! There is no in-tree inference and no model download path here — subprocess
//! backends provision their own files when spawned.

mod discovery;
mod idle;
mod instantiate;
mod lifecycle;
mod switch;

/// The answer to a request whose load gave up for a newer one, which loads
/// what the user asked for last.
pub(in crate::tts::daemon) fn superseded(
    action: &str,
    name: &str,
) -> super_voice_shared::tts::models::protocol::DaemonResponse {
    let msg = format!("Model {action} failed: loading {name} was superseded by a newer model load");
    log::warn!("{msg}");
    super_voice_shared::tts::models::protocol::DaemonResponse::error(&msg)
}
