// SPDX-License-Identifier: GPL-3.0-only
//! The interface font on macOS: the system font, San Francisco.
//!
//! libcosmic draws text in the interface font named by COSMIC's toolkit
//! config, which is Open Sans unless set — the font COSMIC ships, and which
//! libcosmic bundles for everywhere else. On a Mac the app names the system
//! font instead. The text engine finds it in `/System/Library/Fonts` like any
//! installed font, so nothing is bundled, and it is one variable font that
//! covers every weight.
//!
//! The font is stored in the toolkit config rather than only set in memory:
//! once the app is running, libcosmic loads that config into memory again,
//! and would put Open Sans back.

use cosmic::config::{CosmicTk, FontConfig};
use cosmic::cosmic_config::{ConfigGet, ConfigSet};
use cosmic::iced::font::{Stretch, Style, Weight};

/// The toolkit config's key for the interface font.
const INTERFACE_FONT: &str = "interface_font";

/// The family name the text engine knows the system font by, from
/// `SFNS.ttf`. Were it ever missing, text would fall back to Open Sans.
const SYSTEM_FONT: &str = "System Font";

/// Make the system font the interface font.
///
/// Must run before libcosmic first reads its toolkit config, which building
/// `cosmic::app::Settings` does.
pub(crate) fn use_system_font() {
    let config = match CosmicTk::config() {
        Ok(config) => config,
        Err(e) => {
            log::warn!("interface font left as libcosmic's: no toolkit config: {e}");
            return;
        }
    };
    let set = config.get::<FontConfig>(INTERFACE_FONT);
    if set.is_ok_and(|font| font.family == SYSTEM_FONT) {
        return;
    }
    let system_font = FontConfig {
        family: SYSTEM_FONT.to_owned(),
        weight: Weight::Normal,
        stretch: Stretch::Normal,
        style: Style::Normal,
    };
    if let Err(e) = config.set(INTERFACE_FONT, system_font) {
        log::warn!("interface font left as libcosmic's: {e}");
    }
}
