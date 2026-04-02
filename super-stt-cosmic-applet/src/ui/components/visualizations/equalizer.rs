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

/// Bars that are aligned to the bottom of the screen.
pub struct EqualizerVisualization {
    config: VisualizationConfig,
}

impl Default for EqualizerVisualization {
    fn default() -> Self {
        Self {
            config: VisualizationConfig {
                margins: Padding {
                    top: 1.0,
                    right: 2.0,
                    bottom: 0.0,
                    left: 2.0,
                },
                corner_radius: Radius::new(1.0),
                min_element_height: 4.0,
                height_safety_margin: 0.0,
            },
        }
    }
}

impl VisualizationRenderer for EqualizerVisualization {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn draw(
        &self,
        frame: &mut Frame<cosmic::Renderer>,
        bounds: Rectangle,
        frequency_data: &FrequencyData,
        visualization_side: &VisualizationSide,
        color_config: &VisualizationColorConfig,
        is_dark: bool,
        cosmic_theme: &cosmic::cosmic_theme::Theme,
    ) {
        // Use shared config for proper margin handling
        let effective_bounds = self.config.effective_bounds(bounds);
        let total_bars = frequency_data.bands.len().min(32);

        // Determine which bars to show based on VisualizationSide
        let (bars_to_show, bar_start_index) = match visualization_side {
            VisualizationSide::Left => (total_bars / 2, 0), // Show first half
            VisualizationSide::Right => (total_bars / 2, total_bars / 2), // Show second half
            VisualizationSide::Full => (total_bars, 0),     // Show all bars
        };

        let bar_width = effective_bounds.width / bars_to_show as f32 * 0.8;
        let spacing = effective_bounds.width / bars_to_show as f32 * 0.2;

        // Center the bars in the available width
        let total_bars_width =
            (bar_width * bars_to_show as f32) + (spacing * (bars_to_show - 1) as f32);
        let start_x = effective_bounds.x + (effective_bounds.width - total_bars_width) / 2.0;

        // Use configurable normalization from config.rs
        let normalization_factor = 1.0 / FREQUENCY_NORMALIZATION_MAX;

        for display_bar in 0..bars_to_show {
            let x = start_x + (display_bar as f32 * (bar_width + spacing));

            // Map to the correct frequency band index based on wave_side
            let band_index = bar_start_index + display_bar;
            let average_amplitude = if band_index < frequency_data.bands.len() {
                frequency_data.bands[band_index]
            } else {
                0.0
            };

            // Normalize based on the actual maximum value we're receiving
            let height_factor = average_amplitude * normalization_factor;

            // Use shared config for height calculation based on effective bounds
            let max_height = self.config.max_element_height(effective_bounds.height);
            let capped_height_factor = height_factor.min(1.0); // Never exceed 1.0
            let bar_height = max_height * capped_height_factor;
            let clamped_height = self
                .config
                .clamped_element_height(bar_height, effective_bounds.height);

            // Bottom-aligned bars within effective bounds
            let y = effective_bounds.y + effective_bounds.height - clamped_height;

            // Draw all bars
            let mut path_builder = path::Builder::new();
            path_builder.rounded_rectangle(
                Point { x, y },
                cosmic::iced::Size::new(bar_width, clamped_height),
                self.config.corner_radius,
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
}
