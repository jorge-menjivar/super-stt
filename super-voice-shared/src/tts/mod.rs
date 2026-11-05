// SPDX-License-Identifier: GPL-3.0-only
//! Text to speech's own shared code: its wire protocol, the cloned-voice
//! library's types, its speak calls, and its audio frames and resampler.
//!
//! Speech to text's is the rest of the crate, at its root, as in the daemon.
//! What both features share — sessions, scopes, paths, the status — is there
//! too, once.
pub mod daemon;
pub mod models;
pub mod utils;
pub mod validation;

pub mod audio;

// Explicit, non-shadowing re-export. A blanket `pub use models::*` would lift
// the `models` submodule names to this module, where they would be shadowed by
// its own `audio`. Consumers reach model types via their full `models::<sub>`
// paths; only `theme` is used here, so re-export just that.
pub use models::theme;

#[cfg(feature = "audio")]
pub use utils::audio as audio_utils;

pub use audio::*;
