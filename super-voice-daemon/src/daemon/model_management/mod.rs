// SPDX-License-Identifier: GPL-3.0-only
//! Model selection and loading.
//!
//! The daemon is a backend orchestrator: it discovers backends on disk
//! ([`crate::stt_models::backends`]) and routes `(name, source)` to
//! the backend that serves the model, instantiating it as a `dyn Transcribe`.
//! There is no in-tree inference and no model download path here — subprocess
//! backends provision their own files when spawned.

mod discovery;
pub(crate) mod idle;
mod instantiate;
mod lifecycle;
mod post_processor;
mod switch;

/// The answer to a request whose load gave up for a newer one, which loads
/// what the user asked for last.
pub(in crate::daemon) fn superseded(
    action: &str,
    name: &str,
) -> super_voice_shared::models::protocol::DaemonResponse {
    let msg = format!("Model {action} failed: loading {name} was superseded by a newer model load");
    log::warn!("{msg}");
    super_voice_shared::models::protocol::DaemonResponse::error(&msg)
}
