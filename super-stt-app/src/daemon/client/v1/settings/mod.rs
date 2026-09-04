// SPDX-License-Identifier: GPL-3.0-only
//! `/settings` — the daemon's stored preferences.
//!
//! Mirrors the daemon's `v1/settings/`. What lives here is decided by subject,
//! not by scope: the `settings` scope also guards [`super::backends`],
//! [`super::pipeline`] and [`super::registry`], and the app's token carries it
//! for all of them.
//!
//! Neighbours that are about settings without being one: [`super::gpu_info`],
//! [`super::models`], [`super::update`].

pub(crate) mod audio_theme;
pub(crate) mod custom_models_dir;
pub(crate) mod language;
pub(crate) mod notification_method;
pub(crate) mod preview_typing;
pub(crate) mod recording_stop_mode;
pub(crate) mod update_beta_optin;
pub(crate) mod update_check_enabled;
pub(crate) mod volume;
pub(crate) mod write_method;
