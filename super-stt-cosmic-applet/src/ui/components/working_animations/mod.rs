// SPDX-License-Identifier: GPL-3.0-only
pub mod comet;
pub mod dots;
pub mod droplet;

pub use comet::CometAnimation;
pub use dots::DotsAnimation;
pub use droplet::DropletAnimation;

use cosmic::{Renderer, iced::core::Rectangle, iced::widget::canvas::Frame};

use crate::models::theme::{VisualizationColorConfig, VisualizationSide, WorkingAnimationTheme};

/// Per-frame inputs for a working animation. Time-driven (no audio).
#[derive(Clone, Copy)]
pub struct WorkingDrawContext<'a> {
    pub bounds: Rectangle,
    /// Wall-clock milliseconds since the Processing phase began.
    pub elapsed_ms: f32,
    pub color_config: &'a VisualizationColorConfig,
    pub is_dark: bool,
    pub cosmic_theme: &'a cosmic::cosmic_theme::Theme,
    /// Which portion of the (logical full-width) animation this applet renders.
    pub side: &'a VisualizationSide,
}

/// The logical full-width the animation is defined over for a given `side`,
/// and the logical x-coordinate of this applet's left edge within that span.
///
/// For `Full` the logical width is the applet width (x-offset 0). For the side
/// applets the animation is defined over twice the applet width — `Left`
/// renders the `[0, width)` slice and `Right` the `[width, 2*width)` slice — so
/// the two side applets placed inner-edge-to-inner-edge form one continuous
/// animation split at the middle.
pub(crate) fn logical_span(side: &VisualizationSide, width: f32) -> (f32, f32) {
    match side {
        VisualizationSide::Full => (width, 0.0),
        VisualizationSide::Left => (2.0 * width, 0.0),
        VisualizationSide::Right => (2.0 * width, width),
    }
}

/// A time-driven "working" animation renderer.
pub trait WorkingAnimationRenderer {
    fn draw(&self, frame: &mut Frame<Renderer>, ctx: &WorkingDrawContext);
}

/// Smoothstep ease in `[edge0, edge1]`.
pub(crate) fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let u = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

/// Draw the animation for `theme` into `frame`.
pub fn draw(theme: WorkingAnimationTheme, frame: &mut Frame<Renderer>, ctx: &WorkingDrawContext) {
    match theme {
        WorkingAnimationTheme::Droplet => DropletAnimation.draw(frame, ctx),
        WorkingAnimationTheme::Comet => CometAnimation.draw(frame, ctx),
        WorkingAnimationTheme::Dots => DotsAnimation.draw(frame, ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::logical_span;
    use crate::models::theme::VisualizationSide;

    #[test]
    fn logical_span_splits_sides_at_the_middle() {
        // Full: animation spans the applet width, no offset.
        assert_eq!(logical_span(&VisualizationSide::Full, 100.0), (100.0, 0.0));
        // Left: left half of a double-width animation (origin/seam at its right edge).
        assert_eq!(logical_span(&VisualizationSide::Left, 100.0), (200.0, 0.0));
        // Right: right half (its left edge sits at the logical middle = the seam).
        assert_eq!(
            logical_span(&VisualizationSide::Right, 100.0),
            (200.0, 100.0)
        );
    }
}
