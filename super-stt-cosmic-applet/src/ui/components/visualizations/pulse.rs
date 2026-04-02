// SPDX-License-Identifier: GPL-3.0-only
use crate::config::FREQUENCY_NORMALIZATION_MAX;
use crate::models::theme::{VisualizationColorConfig, VisualizationSide};
use crate::ui::components::visualizations::{VisualizationConfig, VisualizationRenderer};
use cosmic::iced::{Padding, Radius};
use cosmic::iced::{
    Point,
    core::Rectangle,
    widget::canvas::{Fill, Frame, path, stroke},
};
use super_stt_shared::FrequencyData;

/// A horizontal line pulse that grows in height/thickness with audio intensity
pub struct PulseVisualization {
    config: VisualizationConfig,
}

impl Default for PulseVisualization {
    fn default() -> Self {
        Self {
            config: VisualizationConfig {
                margins: Padding {
                    top: 2.0,
                    right: 2.0,
                    bottom: 2.0,
                    left: 2.0,
                },
                corner_radius: Radius::new(0.0),
                min_element_height: 4.0,
                height_safety_margin: 1.0,
            },
        }
    }
}

impl VisualizationRenderer for PulseVisualization {
    #[allow(clippy::cast_precision_loss)]
    fn draw(
        &self,
        frame: &mut Frame<cosmic::Renderer>,
        bounds: Rectangle,
        frequency_data: &FrequencyData,
        _side: &VisualizationSide,
        color_config: &VisualizationColorConfig,
        is_dark: bool,
        cosmic_theme: &cosmic::cosmic_theme::Theme,
    ) {
        // Use optimized bounds with minimal margins
        let effective_bounds = self.config.effective_bounds(bounds);

        // Calculate overall audio intensity from frequency data
        let normalization_factor = 1.0 / FREQUENCY_NORMALIZATION_MAX;

        // Focus on vocal frequencies for pulse intensity (bands 8-24)
        let vocal_start = 8;
        let vocal_end = 24;
        let mut vocal_energy = 0.0;
        let mut vocal_bands = 0;

        for i in vocal_start..vocal_end.min(frequency_data.bands.len()) {
            vocal_energy += frequency_data.bands[i];
            vocal_bands += 1;
        }

        let average_vocal_energy = if vocal_bands > 0 {
            vocal_energy / vocal_bands as f32
        } else {
            frequency_data.total_energy
        };

        // Normalize and create pulse intensity
        let pulse_intensity = (average_vocal_energy * normalization_factor).min(1.0);

        // The line always spans the full width of the effective bounds
        let line_width = effective_bounds.width;

        // Calculate line height/thickness based on audio intensity
        let min_height = 2.0; // Minimum visible height when silent
        let max_height = effective_bounds.height * 0.8; // Use up to 80% of available height

        // Create pulsing height based on audio intensity
        let line_height = min_height + (max_height - min_height) * pulse_intensity;

        // Position: always start at left edge (x), centered vertically
        let x = effective_bounds.x;
        let y = effective_bounds.y + (effective_bounds.height - line_height) / 2.0; // Center vertically

        // Draw the full-width horizontal line with rounded ends
        let border_radius = line_height / 2.0; // Make the ends fully rounded (pill shape)
        let mut path_builder = path::Builder::new();
        path_builder.rounded_rectangle(
            Point { x, y },
            cosmic::iced::Size::new(line_width, line_height),
            border_radius.into(),
        );

        let base = color_config.get_color_with_theme(is_dark, cosmic_theme);

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
