// SPDX-License-Identifier: GPL-3.0-only
//! Model selection and loading.
//!
//! The daemon is a backend orchestrator: it discovers backends on disk
//! ([`crate::stt_models::backends`]) and routes `(name, source)` to
//! the backend that serves the model, instantiating it as a `dyn Transcribe`.
//! There is no in-tree inference and no model download path here — subprocess
//! backends provision their own files when spawned.

mod discovery;
mod instantiate;
mod lifecycle;
mod switch;
