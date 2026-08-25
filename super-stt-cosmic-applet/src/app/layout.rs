// SPDX-License-Identifier: GPL-3.0-only
//! Geometry of the applet surface.
//!
//! The panel sizes its bar around the largest applet it hosts, so an applet
//! that renders past the size the panel suggests drags the whole bar with it.
//! Every state this applet can render — the idle icon, the recording
//! visualization, the working animation — therefore resolves to the same box
//! here, and that box never exceeds the panel's suggested window size on the
//! cross axis (the panel's thickness).

use cosmic::applet::Context;
use cosmic::iced::{Padding, Size};

use crate::util::u32_to_f32;

/// Upper bound on the content's cross-axis extent. Panels can be configured
/// arbitrarily thick; past this point extra thickness stops making the
/// visualization more readable.
const MAX_CONTENT_CROSS: f32 = 108.0;

/// Floor for the visualization's length along the panel, matching the low end
/// of the "Visualization Size" slider.
const MIN_CONTENT_MAJOR: f32 = 60.0;

/// How many times the panel's thickness the visualization may run along the
/// panel. Keeps a large configured size from swallowing a thin panel, up to
/// the point where `MIN_CONTENT_MAJOR` takes over: below that width there is
/// nothing worth drawing.
const MAX_MAJOR_CROSS_RATIO: f32 = 8.0;

/// The applet surface, split into the box the panel sees and the drawable
/// area inside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppletLayout {
    /// Size of the whole applet surface, identical in every state.
    pub total: Size,
    /// Drawable area left once `padding` is taken off `total`.
    pub content: Size,
    /// Gap between `content` and the edges of `total`.
    pub padding: Padding,
    /// Whether the panel runs horizontally. The major axis — the one the
    /// visualization stretches along and the icon aligns on — is `total`'s
    /// width when set and its height otherwise.
    pub horizontal: bool,
}

impl AppletLayout {
    /// Resolve the applet's box from the panel's suggestions and the user's
    /// visualization preferences.
    pub fn compute(applet: &Context, show_visualization: bool, visualization_length: u32) -> Self {
        let (suggested_width, suggested_height) = applet.suggested_window_size();
        // `suggested_padding` reports (major axis, cross axis): the major axis
        // runs along the panel, the cross axis across its thickness. The
        // non-symbolic pair is the one to ask for, since this applet's artwork
        // is full-color rather than a symbolic glyph. Both flags describe the
        // same slot — `icon_size(flag) + 2 * padding(flag)` is one panel unit
        // either way, give or take a pixel of truncation on a custom size — so
        // taking this padding off the suggested window leaves the icon slot.
        let (major_padding, icon_slot_padding) = applet.suggested_padding(false);
        let major_padding = f32::from(major_padding);
        let horizontal = applet.is_horizontal();

        // The cross axis is the panel's thickness. Spending exactly this much
        // of it in every state is what stops the bar from resizing as the
        // applet swaps between its icon and its visualizations.
        let suggested_cross = if horizontal {
            u32_to_f32(suggested_height.get())
        } else {
            u32_to_f32(suggested_width.get())
        };
        let content_cross =
            (suggested_cross - 2.0 * f32::from(icon_slot_padding)).clamp(1.0, MAX_CONTENT_CROSS);
        // Anything the clamps took off the content goes back into the padding,
        // so the box spans the reserved thickness even on a panel thick enough
        // (or thin enough) to hit them.
        let cross_padding = ((suggested_cross - content_cross) / 2.0).max(0.0);

        let content_major = if show_visualization {
            u32_to_f32(visualization_length)
                .min(content_cross * MAX_MAJOR_CROSS_RATIO)
                .max(MIN_CONTENT_MAJOR)
        } else {
            // Icon-only renders a square; the glyph inside is sized separately.
            content_cross
        };

        let (content, padding) = if horizontal {
            (
                Size::new(content_major, content_cross),
                Padding::from([cross_padding, major_padding]),
            )
        } else {
            (
                Size::new(content_cross, content_major),
                Padding::from([major_padding, cross_padding]),
            )
        };

        Self {
            total: Size::new(
                content.width + padding.left + padding.right,
                content.height + padding.top + padding.bottom,
            ),
            content,
            padding,
            horizontal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::applet::cosmic_panel_config::{CosmicPanelBackground, PanelAnchor, PanelSize};
    use cosmic::applet::{PanelType, Size as AppletSize};

    /// Every size a panel can be configured with, including the custom sizes
    /// that reach the `MAX_CONTENT_CROSS` clamp from either end.
    fn sizes() -> Vec<PanelSize> {
        vec![
            PanelSize::XS,
            PanelSize::S,
            PanelSize::M,
            PanelSize::L,
            PanelSize::XL,
            PanelSize::Custom(16),
            PanelSize::Custom(64),
            PanelSize::Custom(400),
        ]
    }

    fn context(size: PanelSize, anchor: PanelAnchor) -> Context {
        Context {
            size: AppletSize::PanelSize(size),
            anchor,
            spacing: 4,
            background: CosmicPanelBackground::ThemeDefault,
            output_name: String::new(),
            panel_type: PanelType::Panel,
            suggested_bounds: None,
            padding_overlap: 0.0,
        }
    }

    /// Extent of `size` across the panel, i.e. along the panel's thickness.
    fn cross(size: Size, horizontal: bool) -> f32 {
        if horizontal { size.height } else { size.width }
    }

    /// Pixel measures are whole or half numbers here, so a hair of tolerance
    /// keeps the float-comparison lint happy without weakening the check.
    #[track_caller]
    fn assert_px(actual: f32, expected: f32, context: &str) {
        assert!(
            (actual - expected).abs() < 0.001,
            "{context}: expected {expected}px, got {actual}px",
        );
    }

    /// The panel reserves `icon size + 2 * padding` of thickness per applet
    /// (`cosmic-panel`'s applet size unit). Falling short of it leaves the
    /// applet floating in the bar; overshooting it thickens the bar for
    /// everyone. Expectations come from the `PanelSize` tables directly rather
    /// than from `suggested_window_size`, so a mistake in the axis or padding
    /// this module picks cannot cancel itself out.
    #[test]
    fn fills_the_panel_thickness_exactly() {
        for anchor in [PanelAnchor::Top, PanelAnchor::Left] {
            for size in sizes() {
                for show_visualization in [false, true] {
                    let applet = context(size.clone(), anchor);
                    let layout = AppletLayout::compute(&applet, show_visualization, 120);
                    assert_px(
                        cross(layout.total, applet.is_horizontal()),
                        u32_to_f32(size.get_applet_icon_size_with_padding(true)),
                        &format!("{size:?}/{anchor:?} visualization={show_visualization}"),
                    );
                }
            }
        }
    }

    /// Inside that thickness, the drawable area is the panel's icon slot — the
    /// same room a plain panel icon gets.
    #[test]
    fn content_gets_the_panels_icon_slot() {
        for anchor in [PanelAnchor::Top, PanelAnchor::Left] {
            for size in sizes() {
                let slot = u32_to_f32(size.get_applet_icon_size(false));
                if slot > MAX_CONTENT_CROSS {
                    continue; // Deliberately capped; see `fills_the_panel_thickness_exactly`.
                }
                // A custom size runs through three separate integer
                // divisions (one icon, two paddings), so the symbolic and
                // non-symbolic slots can land up to 2px apart there.
                let tolerance = if matches!(size, PanelSize::Custom(_)) {
                    2.0
                } else {
                    0.001
                };
                let applet = context(size.clone(), anchor);
                let layout = AppletLayout::compute(&applet, true, 120);
                let actual = cross(layout.content, applet.is_horizontal());
                assert!(
                    (actual - slot).abs() <= tolerance,
                    "{size:?}/{anchor:?}: expected {slot}px (+/-{tolerance}), got {actual}px",
                );
            }
        }
    }

    /// A compositor-supplied thickness overrides the panel size, including one
    /// thinner than the padding the panel size asks for.
    #[test]
    fn respects_a_compositor_supplied_thickness() {
        for thickness in [10.0, 44.0] {
            let mut applet = context(PanelSize::M, PanelAnchor::Top);
            applet.suggested_bounds = Some(Size::new(0.0, thickness));
            let layout = AppletLayout::compute(&applet, true, 120);
            assert_px(
                layout.total.height,
                thickness,
                &format!("compositor thickness {thickness}"),
            );
        }
    }

    /// Concrete numbers for the default panel size, so a regression shows up
    /// as the pixel count it is.
    #[test]
    fn medium_horizontal_panel_matches_suggested_window() {
        let applet = context(PanelSize::M, PanelAnchor::Top);
        let layout = AppletLayout::compute(&applet, true, 120);

        // 28px symbolic icon + 2 * 14px cross padding.
        assert_px(layout.total.width, 144.0, "total width");
        assert_px(layout.total.height, 56.0, "total height");
        assert_px(layout.content.width, 120.0, "content width");
        assert_px(layout.content.height, 40.0, "content height");
    }

    /// The same panel on its side: the configured size runs down the panel and
    /// the thickness is now the width.
    #[test]
    fn medium_vertical_panel_swaps_the_axes() {
        let applet = context(PanelSize::M, PanelAnchor::Left);
        let layout = AppletLayout::compute(&applet, true, 120);

        assert!(!layout.horizontal);
        assert_px(layout.total.width, 56.0, "total width");
        assert_px(layout.total.height, 144.0, "total height");
        assert_px(layout.content.width, 40.0, "content width");
        assert_px(layout.content.height, 120.0, "content height");
    }

    /// Whatever the state, the drawable area plus its padding is the box the
    /// panel was told about.
    #[test]
    fn padding_accounts_for_the_difference() {
        for anchor in [PanelAnchor::Top, PanelAnchor::Left] {
            for size in sizes() {
                for show_visualization in [false, true] {
                    let applet = context(size.clone(), anchor);
                    let layout = AppletLayout::compute(&applet, show_visualization, 120);
                    assert_px(
                        layout.content.width + layout.padding.left + layout.padding.right,
                        layout.total.width,
                        &format!("{size:?}/{anchor:?} padded width"),
                    );
                    assert_px(
                        layout.content.height + layout.padding.top + layout.padding.bottom,
                        layout.total.height,
                        &format!("{size:?}/{anchor:?} padded height"),
                    );
                }
            }
        }
    }
}
