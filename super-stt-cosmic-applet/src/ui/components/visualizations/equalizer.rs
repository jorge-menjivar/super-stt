// SPDX-License-Identifier: GPL-3.0-only
use crate::config::FREQUENCY_NORMALIZATION_MAX;
use crate::models::theme::VisualizationSide;
use crate::ui::components::visualizations::{
    DrawContext, VisualizationConfig, VisualizationRenderer,
};
use crate::util::usize_to_f32;
use cosmic::iced::{
    Padding, Point, Radius,
    widget::canvas::{Fill, Frame, path, stroke},
};

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
    fn draw(&self, frame: &mut Frame<cosmic::Renderer>, ctx: &DrawContext) {
        let DrawContext {
            bounds,
            frequency_data,
            side,
            color_config,
            is_dark,
            cosmic_theme,
        } = *ctx;
        let effective_bounds = self.config.effective_bounds(bounds);
        let total_bars = frequency_data.bands.len().min(32);

        let (bars_to_show, bar_start_index) = match side {
            VisualizationSide::Left => (total_bars / 2, 0),
            VisualizationSide::Right => (total_bars / 2, total_bars / 2),
            VisualizationSide::Full => (total_bars, 0),
        };

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

        for display_bar in 0..bars_to_show {
            let x = start_x + (usize_to_f32(display_bar) * (bar_width + spacing));

            let band_index = bar_start_index + display_bar;
            let average_amplitude = if band_index < frequency_data.bands.len() {
                frequency_data.bands[band_index]
            } else {
                0.0
            };

            let height_factor = average_amplitude * normalization_factor;
            let max_height = self.config.max_element_height(effective_bounds.height);
            let capped_height_factor = height_factor.min(1.0);
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
