// SPDX-License-Identifier: GPL-3.0-only

//! The daemon's installed-backend catalog (`GET /backends`).
//!
//! The response shape is shared with the daemon (which serializes it) via
//! [`super_stt_shared::models::backends`], so the two sides cannot drift. This
//! module re-exports it under the names the app's views already use.

pub use super_stt_shared::models::backends::{BackendInfo, BackendOption, BackendSecret};

// `BackendModel` is named only by the models-view test fixtures, never by
// non-test code (views iterate `BackendInfo::models` without naming the type),
// so the re-export reads as unused in a normal build of this binary crate.
#[allow(unused_imports)]
pub use super_stt_shared::models::backends::BackendModel;
