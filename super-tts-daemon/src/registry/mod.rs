// SPDX-License-Identifier: GPL-3.0-only
//! Daemon-side registry client, compatibility evaluation, and install pipeline.
//!
//! The index client, its policy, host detection, build selection and the
//! install record are `super_engine_daemon::registry`'s, shared with Super
//! STT; the modules of those names here bind them to Super TTS.

pub mod carry_over;
pub mod client;
pub mod compat;
pub mod custom_repo;
pub mod host_detect;
pub mod index_schema;
pub mod install;
pub mod installed;
pub mod local_dir;
pub mod reconcile;

/// The directory name a backend installs into. See
/// `super_engine_daemon::registry::install_dir_name`.
pub use super_engine_daemon::registry::install_dir_name;

/// This daemon, as the shared registry code needs to know it.
pub const DAEMON: super_engine_daemon::registry::Daemon = super_engine_daemon::registry::Daemon {
    product: &super_tts_shared::SUPER_TTS,
    version: env!("CARGO_PKG_VERSION"),
    user_agent: super_tts_registry_types::Tts::USER_AGENT,
};
