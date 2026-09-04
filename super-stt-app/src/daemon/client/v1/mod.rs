// SPDX-License-Identifier: GPL-3.0-only
//! `/v1` — one module per path, named for the path it answers on.
//!
//! Mirrors the daemon's `http/v1/` tree: a directory where a path has
//! sub-resources worth separating ([`backends`], [`pipeline`], [`registry`]); a
//! file otherwise, holding that path and any sub-path small enough to read
//! beside it. [`macros`] is the one module not named for a path, because it is
//! not an endpoint.
//!
//! Not every daemon path is wrapped — only what the settings app calls.

// Must come first: `#[macro_use]` puts the settings macros in scope for every
// module declared after it.
#[macro_use]
mod macros;

pub(crate) mod audio_theme;
pub(crate) mod audio_themes;
pub(crate) mod backends;
pub(crate) mod custom_models_dir;
pub(crate) mod gpu_info;
pub(crate) mod language;
pub(crate) mod models;
pub(crate) mod notification_method;
pub(crate) mod ping;
pub(crate) mod pipeline;
pub(crate) mod preview_typing;
pub(crate) mod recording_stop_mode;
pub(crate) mod registry;
pub(crate) mod transcribe;
pub(crate) mod update;
pub(crate) mod update_beta_optin;
pub(crate) mod update_check_enabled;
pub(crate) mod volume;
pub(crate) mod write_method;
