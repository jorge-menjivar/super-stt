// SPDX-License-Identifier: GPL-3.0-only
//! Which build of a backend this host installs, and which variant of each
//! model file it fetches: `super_engine_daemon::registry::compat`, shared
//! with Super TTS, for Super STT's contract generations.

pub use super_engine_daemon::registry::compat::{
    GFX_FAMILY_FLOOR, Selection, select_files, to_selected_asset,
};

use crate::registry::host_detect::Host;
use crate::registry::index_schema::IndexBackend;

/// Which build of `entry` this host should install. See
/// `super_engine_daemon::registry::compat::select`.
#[must_use]
pub fn select(host: &Host, entry: &IndexBackend) -> Selection {
    super_engine_daemon::registry::compat::select::<super_stt_registry_types::Stt>(
        &super::DAEMON,
        host,
        entry,
    )
}
