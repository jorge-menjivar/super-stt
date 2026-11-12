// SPDX-License-Identifier: GPL-3.0-only
//! The Shortcuts page: what the daemon says about each shortcut.

use super_voice_shared::models::shortcuts::ShortcutList;

/// What the Shortcuts page shows.
#[derive(Debug, Clone, Default)]
pub struct ShortcutsState {
    /// The daemon's last answer. `None` until the page is first shown.
    pub list: Option<ShortcutList>,
}
