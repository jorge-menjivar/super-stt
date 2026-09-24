// SPDX-License-Identifier: GPL-3.0-only
//! Session tokens, kept in the keyring per app:
//! [`super_engine_client::session`], with the product bound to Super STT.

use crate::SUPER_STT;
pub use super_engine_client::session::{AppId, forget, load, obtain, save, with_token};

/// The [`AppId`] Super STT's app `name` keeps its token under, e.g.
/// `app_id("super-stt-app")`.
#[must_use]
pub const fn app_id(name: &'static str) -> AppId {
    AppId::new(&SUPER_STT, name)
}

/// Swap the keyring for an in-memory one when `SUPER_STT_KEYRING_MOCK` is
/// set. See [`super_engine_client::session::install_mock_keyring_if_requested`].
pub fn install_mock_keyring_if_requested() {
    super_engine_client::session::install_mock_keyring_if_requested(&SUPER_STT);
}
