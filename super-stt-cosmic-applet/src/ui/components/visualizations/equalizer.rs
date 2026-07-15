// SPDX-License-Identifier: GPL-3.0-only
use crate::ui::components::visualizations::{
    BarAnchor, DrawContext, VisualizationConfig, VisualizationRenderer, render_bars,
};
use cosmic::iced::{Padding, Radius, widget::canvas::Frame};

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
        render_bars(&self.config, BarAnchor::Bottom, frame, ctx);
    }
}
