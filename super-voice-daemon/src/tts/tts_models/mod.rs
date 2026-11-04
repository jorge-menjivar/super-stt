// SPDX-License-Identifier: GPL-3.0-only
//! TTS model orchestration.
//!
//! The daemon no longer compiles model inference in-tree; every model is
//! served by an out-of-tree backend discovered on disk (see [`backends`]).
//! [`synthesize`] defines the common trait the hosts present; [`wasm`] and
//! [`subprocess`] are the two backend transports. [`download`] provisions a
//! backend's model files before it is spawned.
//!
//! The in-tree Whisper/Voxtral implementations this daemon descends from were
//! moved out to standalone repositories before the TTS fork. They transcribe
//! rather than synthesize, so they are not backends this project can load.
pub mod backends;
pub mod download;
pub mod model_definition;
pub use model_definition::ModelDefinition;
#[cfg(feature = "subprocess-backends")]
pub mod subprocess;
pub mod synthesize;
// The `/v1` request/response *types* are backend-agnostic and are referenced
// from `synthesize`'s trait signature, so the module is always compiled; the
// body pump inside it is what carries the transport-dependent gate.
pub mod v1;
#[cfg(feature = "wasm-backends")]
pub mod wasm;
