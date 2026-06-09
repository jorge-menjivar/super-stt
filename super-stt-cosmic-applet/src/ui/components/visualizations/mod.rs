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
    iced::{Padding, border, core::Rectangle, widget::canvas::Frame},
};

use crate::models::theme::{VisualizationColorConfig, VisualizationSide};
use super_stt_shared::FrequencyData;

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
