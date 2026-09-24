// SPDX-License-Identifier: GPL-3.0-only
//! Self-update checking, shared with Super TTS: `super_engine_daemon::self_update`.
//! Contract: docs/protocol/endpoints/v1/update.md

pub use super_engine_daemon::self_update::*;

/// Super STT's checker, comparing its releases against this build's version.
#[must_use]
pub fn checker() -> SelfUpdateChecker {
    SelfUpdateChecker::new(&super_stt_shared::SUPER_STT, env!("CARGO_PKG_VERSION"))
}
