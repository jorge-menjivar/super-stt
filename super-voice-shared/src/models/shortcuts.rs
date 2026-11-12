// SPDX-License-Identifier: GPL-3.0-only
//! What `/v1/shortcuts` speaks: Super Voice's keyboard shortcuts, and where
//! each one stands on this desktop.
//!
//! The daemon owns the shortcuts, so every client sees the same answer and
//! none has to write desktop settings itself. How it puts a shortcut on the
//! desktop is the [`ShortcutMethod`]; a client shows it, it does not choose it.

use serde::{Deserialize, Serialize};
use super_engine_protocol::wire_enum_strings;

/// One of Super Voice's shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutId {
    /// Record, transcribe, and type what was said: `voice record --write`.
    Dictate,
    /// Read the selected or copied text aloud: `voice speak --read`.
    ReadAloud,
}

wire_enum_strings!(ShortcutId {
    Dictate => "dictate",
    ReadAloud => "read_aloud",
});

impl ShortcutId {
    /// Every shortcut, in the order a list shows them.
    pub const ALL: [Self; 2] = [Self::Dictate, Self::ReadAloud];
}

/// How the daemon puts shortcuts on this desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutMethod {
    /// COSMIC: an entry in its custom-shortcuts file, which runs the command.
    Cosmic,
    /// The desktop's `GlobalShortcuts` portal. The desktop asks the user which
    /// keys to use, and tells the daemon when they are pressed.
    Portal,
    /// Neither. The user binds the command in the desktop's own settings.
    Manual,
}

wire_enum_strings!(ShortcutMethod {
    Cosmic => "cosmic",
    Portal => "portal",
    Manual => "manual",
});

/// Where one shortcut stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutState {
    /// Set up, on [`Shortcut::keys`].
    Installed,
    /// Not set up, and its keys are free: `POST /shortcuts/{id}` adds it.
    Missing,
    /// Not set up, and another shortcut holds its keys. The user binds the
    /// command to other keys.
    Taken,
    /// This desktop has no method the daemon can use. The user binds the
    /// command in the desktop's settings.
    Manual,
}

wire_enum_strings!(ShortcutState {
    Installed => "installed",
    Missing => "missing",
    Taken => "taken",
    Manual => "manual",
});

/// One shortcut on `GET /shortcuts`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Shortcut {
    pub id: ShortcutId,
    pub state: ShortcutState,
    /// The keys, as people write them. `installed`: the keys it is on, which
    /// may be others than its own if the user rebound it. Otherwise the keys
    /// it would take, or that another shortcut holds.
    #[cfg_attr(feature = "openapi", schema(example = "Super+Shift+Space"))]
    pub keys: String,
    /// What the shortcut runs, for binding it by hand.
    #[cfg_attr(feature = "openapi", schema(example = "voice speak --read"))]
    pub command: String,
}

/// What `/shortcuts` answers with: the method, and every shortcut.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ShortcutList {
    /// Always `success`.
    #[cfg_attr(feature = "openapi", schema(example = "success"))]
    pub status: String,
    pub method: ShortcutMethod,
    /// Every shortcut, in [`ShortcutId::ALL`]'s order.
    pub shortcuts: Vec<Shortcut>,
}
