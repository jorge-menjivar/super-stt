// SPDX-License-Identifier: GPL-3.0-only

//! Phosphor icons (regular weight) embedded for the settings UI.
//!
//! Source: <https://github.com/phosphor-icons/core> — SVGs use `currentColor`,
//! so they pick up the active theme via the symbolic flag.

use cosmic::widget::icon::{self, Icon};

pub const GEAR: &[u8] = include_bytes!("../../resources/icons/phosphor/gear.svg");
pub const MICROPHONE: &[u8] = include_bytes!("../../resources/icons/phosphor/microphone.svg");
pub const KEYBOARD: &[u8] = include_bytes!("../../resources/icons/phosphor/keyboard.svg");
pub const BRAIN: &[u8] = include_bytes!("../../resources/icons/phosphor/brain.svg");
pub const CLOUD: &[u8] = include_bytes!("../../resources/icons/phosphor/cloud.svg");
pub const PLUG: &[u8] = include_bytes!("../../resources/icons/phosphor/plug.svg");
pub const CARET_RIGHT: &[u8] = include_bytes!("../../resources/icons/phosphor/caret-right.svg");
pub const WARNING: &[u8] = include_bytes!("../../resources/icons/phosphor/warning.svg");
pub const CHECK: &[u8] = include_bytes!("../../resources/icons/phosphor/check.svg");

/// Build a themable [`Icon`] from one of the embedded Phosphor SVGs.
pub fn phosphor(bytes: &'static [u8]) -> Icon {
    icon::from_svg_bytes(bytes).symbolic(true).icon()
}
