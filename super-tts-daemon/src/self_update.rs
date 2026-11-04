// SPDX-License-Identifier: GPL-3.0-only
//! Self-update checking, shared with Super STT: `super_engine_daemon::self_update`.
//! Contract: docs/protocol/endpoints/v1/update.md

pub use super_engine_daemon::self_update::*;

/// Super TTS's checker, comparing its releases against this build's version.
#[must_use]
pub fn checker() -> SelfUpdateChecker {
    SelfUpdateChecker::new(&super_tts_shared::SUPER_TTS, env!("CARGO_PKG_VERSION"))
}
