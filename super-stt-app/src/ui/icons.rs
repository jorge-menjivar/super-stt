// SPDX-License-Identifier: GPL-3.0-only

//! Phosphor icons (regular weight) embedded for the settings UI.
//!
//! Source: <https://github.com/phosphor-icons/core> — SVGs use `currentColor`,
//! so they pick up the active theme via the symbolic flag.

use cosmic::iced::Length;
use cosmic::iced::widget::svg;
use cosmic::widget::icon::{self, Icon};

pub const GEAR: &[u8] = include_bytes!("../../resources/icons/phosphor/gear.svg");
pub const MICROPHONE: &[u8] = include_bytes!("../../resources/icons/phosphor/microphone.svg");
pub const KEYBOARD: &[u8] = include_bytes!("../../resources/icons/phosphor/keyboard.svg");
pub const BRAIN: &[u8] = include_bytes!("../../resources/icons/phosphor/brain.svg");
pub const PLUG: &[u8] = include_bytes!("../../resources/icons/phosphor/plug.svg");
pub const WARNING: &[u8] = include_bytes!("../../resources/icons/phosphor/warning.svg");
pub const CPU: &[u8] = include_bytes!("../../resources/icons/phosphor/cpu.svg");
pub const GRAPHICS_CARD: &[u8] = include_bytes!("../../resources/icons/phosphor/graphics-card.svg");
pub const CLOUD: &[u8] = include_bytes!("../../resources/icons/phosphor/cloud.svg");
pub const DOTS_THREE_VERTICAL: &[u8] =
    include_bytes!("../../resources/icons/phosphor/dots-three-vertical.svg");
pub const ARROWS_CLOCKWISE: &[u8] =
    include_bytes!("../../resources/icons/phosphor/arrows-clockwise.svg");
pub const PLAY: &[u8] = include_bytes!("../../resources/icons/phosphor/play.svg");
pub const STOP: &[u8] = include_bytes!("../../resources/icons/phosphor/stop.svg");
pub const GIT_BRANCH: &[u8] = include_bytes!("../../resources/icons/phosphor/git-branch.svg");
pub const BOOKS: &[u8] = include_bytes!("../../resources/icons/phosphor/books.svg");

/// The Super STT app logo, full-color artwork. Not a Phosphor glyph, so it
/// lives at the app resources root rather than the phosphor set; shown beside
/// the app name in the window header.
pub const APP_LOGO: &[u8] = include_bytes!("../../resources/super-stt-icon.svg");

/// Build a themable [`Icon`] from one of the embedded Phosphor SVGs.
pub fn phosphor(bytes: &'static [u8]) -> Icon {
    icon::from_svg_bytes(bytes).symbolic(true).icon()
}

/// The symbolic [`Handle`](icon::Handle) for one of the embedded Phosphor SVGs,
/// for widgets that take a handle directly (e.g. `button::icon`).
pub fn phosphor_handle(bytes: &'static [u8]) -> icon::Handle {
    icon::from_svg_bytes(bytes).symbolic(true)
}

/// Shared builder for a fixed-size symbolic Phosphor [`Svg`](cosmic::widget::Svg)
/// tinted by a caller-supplied [`Svg`](cosmic::theme::Svg) color class. The
/// plain [`Icon`] wrapper hides the `svg::Style::color` knob that does the
/// tinting, so the tinted variants return the bare `Svg` widget instead.
fn tinted_svg(
    bytes: &'static [u8],
    size: f32,
    class: cosmic::theme::Svg,
) -> cosmic::widget::Svg<'static, cosmic::Theme> {
    cosmic::widget::Svg::<cosmic::Theme>::new(svg::Handle::from_memory(bytes))
        .symbolic(true)
        .class(class)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
}

/// A Phosphor icon tinted with the theme's *destructive* (red) color — used
/// for unmet-requirement warnings inside a backend card.
pub fn phosphor_destructive(
    bytes: &'static [u8],
    size: f32,
) -> cosmic::widget::Svg<'static, cosmic::Theme> {
    tinted_svg(
        bytes,
        size,
        cosmic::theme::Svg::custom(|t| svg::Style {
            color: Some(t.cosmic().destructive.base.into()),
        }),
    )
}

/// A Phosphor icon tinted with the theme's *warning* (yellow) color — used
/// for the advisory "model may not fit in GPU memory" warning.
pub fn phosphor_warning(
    bytes: &'static [u8],
    size: f32,
) -> cosmic::widget::Svg<'static, cosmic::Theme> {
    tinted_svg(
        bytes,
        size,
        cosmic::theme::Svg::custom(|t| svg::Style {
            color: Some(t.cosmic().warning.base.into()),
        }),
    )
}

/// A Phosphor icon tinted with an explicit `color` the caller resolves from
/// the active theme. Used by the backend-capability chips, whose tone
/// (accent / neutral) is chosen at view-build time.
pub fn phosphor_tinted(
    bytes: &'static [u8],
    size: f32,
    color: cosmic::iced::Color,
) -> cosmic::widget::Svg<'static, cosmic::Theme> {
    tinted_svg(
        bytes,
        size,
        cosmic::theme::Svg::custom(move |_| svg::Style { color: Some(color) }),
    )
}

/// The Super STT logo ([`APP_LOGO`]) rendered at `size` px in its own colors.
///
/// The artwork is multi-color, so it is deliberately neither marked symbolic
/// nor given a tinting class: an explicit `svg::Style::color` is applied to the
/// whole image regardless of the symbolic flag, which would flatten the logo to
/// a single-color silhouette.
pub fn app_logo(size: f32) -> cosmic::widget::Svg<'static, cosmic::Theme> {
    cosmic::widget::Svg::<cosmic::Theme>::new(svg::Handle::from_memory(APP_LOGO))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
}
