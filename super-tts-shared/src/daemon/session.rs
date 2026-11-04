// SPDX-License-Identifier: GPL-3.0-only
//! Session tokens, kept in a file per app:
//! [`super_engine_client::session`], with the product bound to Super TTS.

use crate::SUPER_TTS;
pub use super_engine_client::session::{AppId, forget, load, obtain, save, with_token};

/// The [`AppId`] Super TTS's app `name` keeps its token under, e.g.
/// `app_id("super-tts-app")`.
#[must_use]
pub const fn app_id(name: &'static str) -> AppId {
    AppId::new(&SUPER_TTS, name)
}
