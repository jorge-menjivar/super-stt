// SPDX-License-Identifier: GPL-3.0-only
use crate::ui::components::visualizations::{
    BarAnchor, DrawContext, VisualizationConfig, VisualizationRenderer, render_bars,
};
use cosmic::iced::{Padding, Radius, widget::canvas::Frame};

/// Bars that are centered vertically within the drawable bounds.
pub struct CenteredBarsVisualization {
    config: VisualizationConfig,
}

impl Default for CenteredBarsVisualization {
    fn default() -> Self {
        Self {
            config: VisualizationConfig {
                margins: Padding {
                    top: 1.0,
                    right: 0.0,
                    bottom: 1.0,
                    left: 0.0,
                },
                corner_radius: Radius::new(24.0),
                min_element_height: 4.0,
                height_safety_margin: 0.0,
            },
        }
    }
}

impl VisualizationRenderer for CenteredBarsVisualization {
    fn draw(&self, frame: &mut Frame<cosmic::Renderer>, ctx: &DrawContext) {
        render_bars(&self.config, BarAnchor::Center, frame, ctx);
    }
}
