// SPDX-License-Identifier: GPL-3.0-only

//! Phosphor icons (regular weight) embedded for the settings UI.
//!
//! Source: <https://github.com/phosphor-icons/core>, copied verbatim from
//! `raw/regular/` — the stroked source art, not the outlined `assets/regular/`
//! export, which draws the same glyphs as filled paths and sits visibly heavier
//! beside these. Take any new icon from `raw/regular/` too. The SVGs use
//! `currentColor`, so they pick up the active theme via the symbolic flag.

use std::sync::LazyLock;

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
pub const X: &[u8] = include_bytes!("../../resources/icons/phosphor/x.svg");
pub const CIRCLE_NOTCH: &[u8] = include_bytes!("../../resources/icons/phosphor/circle-notch.svg");

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

/// How many angles the [`CIRCLE_NOTCH`] spinner is drawn at.
///
/// Divides 360 exactly, so every frame lands on a whole number of degrees.
pub const SPINNER_FRAME_COUNT: usize = 24;

/// How long each spinner frame holds — [`SPINNER_FRAME_COUNT`] of these make
/// one turn a second.
pub const SPINNER_FRAME: std::time::Duration = std::time::Duration::from_millis(42);

/// [`CIRCLE_NOTCH`] as SVG source, pre-turned to each of the
/// [`SPINNER_FRAME_COUNT`] angles.
///
/// Rotating at draw time is the obvious way and the wrong one here: iced
/// rasterizes an SVG once at its layout size, caches that bitmap under
/// `(id, width, height, color)` — rotation is not in the key — and then spins
/// it in the shader sampling *nearest-neighbour*. An 18px glyph turned that way
/// comes out visibly chewed. Turning the geometry instead means resvg draws
/// each angle from the path with its own antialiasing. The frames are a fixed
/// set of byte strings and the cache keys off their hash, so this costs exactly
/// `SPINNER_FRAME_COUNT` small rasters rather than one per angle ever shown.
static SPINNER_FRAMES: LazyLock<Vec<Vec<u8>>> = LazyLock::new(|| {
    let inner = svg_inner(CIRCLE_NOTCH);
    (0..SPINNER_FRAME_COUNT)
        .map(|i| {
            // Whole degrees: 360 / 24 = 15, so no rounding creeps in and the
            // frame at index 0 is the source art untouched.
            let deg = 360 * i / SPINNER_FRAME_COUNT;
            format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 256 256\">\
                 <g transform=\"rotate({deg} 128 128)\">{inner}</g></svg>"
            )
            .into_bytes()
        })
        .collect()
});

/// The drawing inside an embedded Phosphor SVG, without its root element, so it
/// can be re-wrapped in one carrying a transform.
///
/// Every file in `resources/icons/phosphor/` is one `<svg …>…</svg>` on a
/// single line, which is what makes this a slice rather than a parse.
fn svg_inner(bytes: &'static [u8]) -> &'static str {
    let s = std::str::from_utf8(bytes).expect("embedded Phosphor SVGs are UTF-8");
    let body = s.find('>').map_or(s, |i| &s[i + 1..]);
    body.trim_end().trim_end_matches("</svg>")
}

/// The [`CIRCLE_NOTCH`] spinner at `frame`, which the caller advances on its
/// own tick. Indices past the last frame wrap, so a counter can just count up.
pub fn spinner(
    size: f32,
    color: cosmic::iced::Color,
    frame: usize,
) -> cosmic::widget::Svg<'static, cosmic::Theme> {
    let bytes = SPINNER_FRAMES[frame % SPINNER_FRAME_COUNT].as_slice();
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
