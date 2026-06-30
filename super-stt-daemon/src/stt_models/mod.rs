// SPDX-License-Identifier: GPL-3.0-only
//! STT model orchestration.
//!
//! The daemon no longer compiles model inference in-tree; every model is
//! served by an out-of-tree backend discovered on disk (see [`backends`]).
//! [`transcribe`] defines the common trait the hosts present; [`wasm`] and
//! [`subprocess`] are the two backend transports. [`download`] provisions a
//! backend's model files before it is spawned.
//!
//! The previous in-tree Whisper/Voxtral/online implementations now live in
//! their own standalone backend repositories.
pub mod backends;
pub mod dispatch;
pub mod download;
#[cfg(feature = "subprocess-backends")]
pub mod subprocess;
pub mod transcribe;
#[cfg(feature = "wasm-backends")]
pub mod wasm;
