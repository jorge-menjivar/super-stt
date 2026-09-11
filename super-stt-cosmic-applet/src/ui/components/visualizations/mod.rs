// SPDX-License-Identifier: GPL-3.0-only
pub mod centered_bars;
pub mod equalizer;
pub mod pulse;
pub mod waveform;

pub use centered_bars::CenteredBarsVisualization;
pub use equalizer::EqualizerVisualization;
pub use pulse::PulseVisualization;
pub use waveform::WaveformVisualization;

use cosmic::{
    Renderer,
    iced::{
        Padding, Point, Size, border,
        core::Rectangle,
        widget::canvas::{Fill, Frame, path, stroke},
    },
};

use crate::config::FREQUENCY_NORMALIZATION_MAX;
use crate::models::theme::{VisualizationColorConfig, VisualizationSide};
use crate::util::usize_to_f32;
use super_stt_shared::FrequencyData;

/// Maximum number of frequency bands any bar/waveform visualization renders.
/// Frequency data may carry more; the extras are ignored so bar sizing stays
/// stable across inputs. Previously a bare `32` copy-pasted into every renderer.
pub const MAX_VISUALIZATION_BANDS: usize = 32;

/// Vertical anchoring for a bar within the drawable bounds. The only thing that
/// distinguishes the bottom equalizer from the centered equalizer.
#[derive(Clone, Copy)]
pub enum BarAnchor {
    /// Bars grow upward from the bottom edge (bottom equalizer).
    Bottom,
    /// Bars are centered vertically (centered equalizer).
    Center,
}

/// Split the frequency spectrum for the applet's rendered `side`, returning
/// `(count, start_index)`: how many bands this instance draws and where its
/// slice begins. `Left`/`Right` each render half the (capped) spectrum; `Full`
/// renders all of it. Shared by every bar/waveform renderer so the
/// [`MAX_VISUALIZATION_BANDS`] cap and the half-split live in one place.
pub fn visible_band_range(side: &VisualizationSide, total_bands: usize) -> (usize, usize) {
    let total = total_bands.min(MAX_VISUALIZATION_BANDS);
    match side {
        VisualizationSide::Left => (total / 2, 0),
        VisualizationSide::Right => (total / 2, total / 2),
        VisualizationSide::Full => (total, 0),
    }
}

/// Render the vertical-bar family of visualizations (bottom equalizer and
/// centered equalizer). The two differ only in their per-style
/// [`VisualizationConfig`] and vertical [`BarAnchor`]; all band selection, bar
/// sizing, spacing, centering, and drawing is identical and lives here.
pub fn render_bars(
    config: &VisualizationConfig,
    anchor: BarAnchor,
    frame: &mut Frame<Renderer>,
    ctx: &DrawContext,
) {
    let DrawContext {
        bounds,
        frequency_data,
        side,
        color_config,
        is_dark,
        cosmic_theme,
    } = *ctx;
    let effective_bounds = config.effective_bounds(bounds);
    let (bars_to_show, bar_start_index) = visible_band_range(side, frequency_data.bands.len());

    // Nothing to draw (e.g. malformed/empty frequency data decoded to zero
    // bands). Bail before dividing `bar_width`/`spacing` by zero and before
    // `bars_to_show - 1` underflows the usize below (Tier 1 #21, same class
    // as #268).
    if bars_to_show == 0 {
        return;
    }

    let bars_f32 = usize_to_f32(bars_to_show);
    let bar_width = effective_bounds.width / bars_f32 * 0.8;
    let spacing = effective_bounds.width / bars_f32 * 0.2;

    // Center the bars in the available width.
    let total_bars_width =
        (bar_width * bars_f32) + (spacing * usize_to_f32(bars_to_show.saturating_sub(1)));
    let start_x = effective_bounds.x + (effective_bounds.width - total_bars_width) / 2.0;

    let normalization_factor = 1.0 / FREQUENCY_NORMALIZATION_MAX;

    // Loop-invariant: neither the fill color nor the height ceiling depends on
    // the per-bar amplitude, so resolve them once instead of per bar.
    let base = color_config.get_color_with_theme(is_dark, cosmic_theme);
    let max_height = config.max_element_height(effective_bounds.height);

    for display_bar in 0..bars_to_show {
        let x = start_x + (usize_to_f32(display_bar) * (bar_width + spacing));

        let band_index = bar_start_index + display_bar;
        let average_amplitude = if band_index < frequency_data.bands.len() {
            frequency_data.bands[band_index]
        } else {
            0.0
        };

        let height_factor = average_amplitude * normalization_factor;
        let capped_height_factor = height_factor.min(1.0);
        let bar_height = max_height * capped_height_factor;
        let clamped_height = config.clamped_element_height(bar_height, effective_bounds.height);

        let y = match anchor {
            BarAnchor::Bottom => effective_bounds.y + effective_bounds.height - clamped_height,
            BarAnchor::Center => {
                effective_bounds.y + (effective_bounds.height - clamped_height) / 2.0
            }
        };

        let mut path_builder = path::Builder::new();
        path_builder.rounded_rectangle(
            Point { x, y },
            Size::new(bar_width, clamped_height),
            config.corner_radius,
        );
        let path = path_builder.build();
        frame.fill(
            &path,
            Fill {
                style: stroke::Style::Solid(base),
                ..Default::default()
            },
        );
    }
}

/// Shared configuration for visualization rendering with proper margin and height management
#[derive(Debug, Clone)]
pub struct VisualizationConfig {
    /// Horizontal and vertical margins using Iced's Padding type
    pub margins: Padding,

    /// Corner radius for rounded rectangles
    pub corner_radius: border::Radius,

    /// Minimum height for bars/elements
    pub min_element_height: f32,

    /// Margin from canvas edges specifically for maximum height calculation
    pub height_safety_margin: f32,
}

impl VisualizationConfig {
    /// Calculate effective drawing bounds given canvas bounds
    pub fn effective_bounds(&self, canvas_bounds: Rectangle) -> Rectangle {
        Rectangle {
            x: canvas_bounds.x + self.margins.left,
            y: canvas_bounds.y + self.margins.top,
            width: canvas_bounds.width - (self.margins.left + self.margins.right),
            height: canvas_bounds.height - (self.margins.top + self.margins.bottom),
        }
    }

    /// Calculate maximum safe element height within canvas bounds
    pub fn max_element_height(&self, canvas_height: f32) -> f32 {
        let available_height = canvas_height - self.height_safety_margin;
        available_height.max(1.0) // Minimum 1px height
    }

    /// Calculate minimum element height based on canvas size
    pub fn min_element_height(&self, canvas_height: f32) -> f32 {
        let canvas_min = canvas_height / 4.0;
        canvas_min.min(self.min_element_height).max(1.0)
    }

    /// Get clamped element height between min and max bounds
    pub fn clamped_element_height(&self, desired_height: f32, canvas_height: f32) -> f32 {
        let min_height = self.min_element_height(canvas_height);
        let max_height = self.max_element_height(canvas_height);
        desired_height.max(min_height).min(max_height)
    }
}

/// Per-frame inputs shared by every [`VisualizationRenderer`]. Bundled
/// into one context so renderers take a single argument instead of a
/// long positional parameter list. All fields are `Copy`, so a renderer
/// can destructure `*ctx` directly.
#[derive(Clone, Copy)]
pub struct DrawContext<'a> {
    /// Canvas bounds to draw within.
    pub bounds: Rectangle,
    /// Frequency analysis data for the current frame.
    pub frequency_data: &'a FrequencyData,
    /// Which portion of the spectrum this applet instance renders.
    pub side: &'a VisualizationSide,
    /// User-configured colors.
    pub color_config: &'a VisualizationColorConfig,
    /// Whether the active COSMIC theme is dark.
    pub is_dark: bool,
    /// Active COSMIC theme, for resolving system colors.
    pub cosmic_theme: &'a cosmic::cosmic_theme::Theme,
}

/// Common trait for all visualization renderers.
pub trait VisualizationRenderer {
    /// Draw the visualization using the per-frame [`DrawContext`].
    fn draw(&self, frame: &mut Frame<Renderer>, ctx: &DrawContext);
}

#[cfg(test)]
mod visible_band_range_tests {
    use super::{MAX_VISUALIZATION_BANDS, VisualizationSide, visible_band_range};

    #[test]
    fn full_takes_all_capped_bands_from_the_start() {
        assert_eq!(visible_band_range(&VisualizationSide::Full, 32), (32, 0));
        assert_eq!(visible_band_range(&VisualizationSide::Full, 8), (8, 0));
    }

    #[test]
    fn left_and_right_split_the_spectrum_in_half() {
        assert_eq!(visible_band_range(&VisualizationSide::Left, 32), (16, 0));
        assert_eq!(visible_band_range(&VisualizationSide::Right, 32), (16, 16));
    }

    #[test]
    fn caps_at_max_visualization_bands() {
        // Frequency data can carry more than we render (e.g. 64 bands); the
        // extras are ignored so bar sizing stays stable.
        assert_eq!(
            visible_band_range(&VisualizationSide::Full, 64),
            (MAX_VISUALIZATION_BANDS, 0)
        );
        assert_eq!(
            visible_band_range(&VisualizationSide::Right, 64),
            (MAX_VISUALIZATION_BANDS / 2, MAX_VISUALIZATION_BANDS / 2)
        );
    }

    #[test]
    fn zero_bands_yields_zero_count() {
        // Renderers rely on a zero count to bail before dividing by zero.
        assert_eq!(visible_band_range(&VisualizationSide::Full, 0), (0, 0));
        assert_eq!(visible_band_range(&VisualizationSide::Left, 0), (0, 0));
    }
}

#[cfg(test)]
mod visualization_config_tests {
    //! The height math every bar renderer shares. Its whole job is keeping an
    //! element inside the panel: the applet draws into a fixed-size strip of
    //! someone else's panel, so a bar that overshoots is drawn over its
    //! neighbours rather than clipped.
    use super::VisualizationConfig;
    use cosmic::iced::{Padding, border::Radius, core::Rectangle};

    fn config(min_element_height: f32, height_safety_margin: f32) -> VisualizationConfig {
        VisualizationConfig {
            margins: Padding {
                top: 2.0,
                right: 4.0,
                bottom: 6.0,
                left: 8.0,
            },
            corner_radius: Radius::new(2.0),
            min_element_height,
            height_safety_margin,
        }
    }

    #[test]
    fn effective_bounds_inset_every_margin() {
        let bounds = config(4.0, 0.0).effective_bounds(Rectangle {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        });

        assert_eq!((bounds.x, bounds.y), (18.0, 22.0));
        assert_eq!(bounds.width, 100.0 - (8.0 + 4.0));
        assert_eq!(bounds.height, 50.0 - (2.0 + 6.0));
    }

    #[test]
    fn heights_never_collapse_to_zero() {
        // A panel shorter than the safety margin still has to draw something:
        // a zero height is an invisible bar, not a small one.
        let config = config(4.0, 40.0);

        assert_eq!(config.max_element_height(10.0), 1.0);
        assert_eq!(config.min_element_height(0.0), 1.0);
    }

    #[test]
    fn the_minimum_element_height_is_capped_by_the_canvas() {
        // On a short panel a quarter of the canvas wins over the configured
        // minimum, so a quiet band can never fill the whole applet.
        let config = config(20.0, 0.0);

        assert_eq!(config.min_element_height(40.0), 10.0);
        assert_eq!(config.min_element_height(200.0), 20.0);
    }

    #[test]
    fn clamped_element_height_keeps_a_bar_between_its_floor_and_ceiling() {
        let config = config(4.0, 6.0);
        let canvas = 60.0;

        assert_eq!(
            config.clamped_element_height(1000.0, canvas),
            config.max_element_height(canvas),
        );
        assert_eq!(
            config.clamped_element_height(0.0, canvas),
            config.min_element_height(canvas),
        );
        assert_eq!(config.clamped_element_height(20.0, canvas), 20.0);
    }

    #[test]
    fn the_ceiling_wins_when_it_sits_below_the_floor() {
        // A safety margin that eats the whole canvas leaves the floor above
        // the ceiling. The ceiling has to win, or the bar is drawn outside
        // the panel.
        let config = config(20.0, 100.0);

        assert_eq!(config.clamped_element_height(50.0, 60.0), 1.0);
        assert_eq!(
            config.clamped_element_height(50.0, 60.0),
            config.max_element_height(60.0)
        );
    }
}
