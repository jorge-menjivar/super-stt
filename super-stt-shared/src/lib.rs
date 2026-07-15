// SPDX-License-Identifier: GPL-3.0-only
pub mod daemon;
pub mod logging;
pub mod models;
pub mod paths;
pub mod registry;
pub mod utils;
pub mod validation;

pub mod audio;

// Explicit, non-shadowing re-export. A blanket `pub use models::*` used to lift
// the `models::{registry,audio}` submodule names to the crate root, where they
// were silently shadowed by the top-level `registry`/`audio` modules. Consumers
// reach model types via their full `models::<sub>` paths; only `theme` is used
// at the crate root, so re-export just that.
pub use models::theme;

#[cfg(feature = "audio")]
pub use utils::audio as audio_utils;

pub use audio::*;
