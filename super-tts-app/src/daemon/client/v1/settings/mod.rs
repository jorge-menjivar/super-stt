// SPDX-License-Identifier: GPL-3.0-only
//! `/settings` — the daemon's stored preferences.
//!
//! Mirrors the daemon's `v1/settings/`. What lives here is decided by subject,
//! not by scope: the `settings` scope also guards [`super::backends`],
//! [`super::pipeline`] and [`super::registry`], and the app's token carries it
//! for all of them. Sharing the scope is not the same as being a setting.
//!
//! Neighbours that are about settings without being one: [`super::gpu_info`], a
//! live probe of the machine, and [`super::update`], which reports what a
//! release check found.
//!
//! The two macros that generate most of these wrappers live in `v1::macros`,
//! not here — they answer on no path either.

pub(crate) mod allow_online_models;
pub(crate) mod audio_theme;
pub(crate) mod custom_models_dir;
pub(crate) mod language;
pub(crate) mod notification_method;
pub(crate) mod update_beta_optin;
pub(crate) mod update_check_enabled;
pub(crate) mod volume;
