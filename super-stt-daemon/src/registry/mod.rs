// SPDX-License-Identifier: GPL-3.0-only
//! Daemon-side registry client, compatibility evaluation, and install pipeline.

pub mod client;
pub mod compat;
pub mod custom_repo;
pub mod host_detect;
pub mod index_schema;
pub mod install;
pub mod local_dir;

/// Re-export the shared operator-base-URL gate from `super-stt-forge` so the
/// registry client and the forge adapters apply one identical rule.
pub(crate) use super_stt_forge::accept_base_url;
