// SPDX-License-Identifier: GPL-3.0-only
use crate::config::FREQUENCY_NORMALIZATION_MAX;
use crate::ui::components::visualizations::{VisualizationConfig, VisualizationRenderer};
use cosmic::iced::{Padding, Radius};
use super_stt_shared::FrequencyData;

use crate::models::theme::{VisualizationColorConfig, VisualizationSide};
use cosmic::iced::{
    Point,
    core::Rectangle,
    widget::canvas::{Fill, Frame, path, stroke},
};

/// Bottom-aligned frequency waveform rendering
/// Shows frequency bands as a smooth continuous wave rising from the bottom
pub struct WaveformVisualization {
    config: VisualizationConfig,
}

const SMOOTHING_FACTOR: f32 = 4.0;
const STROKE_WIDTH: f32 = 1.5;
const FILL_OPACITY: f32 = 0.3;

impl Default for WaveformVisualization {
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
                min_element_height: 0.0,
                height_safety_margin: 0.0,
            },
        }
    }
}

impl VisualizationRenderer for WaveformVisualization {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::too_many_lines
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
        // Use effective bounds with margins
        let effective_bounds = self.config.effective_bounds(bounds);

        // Determine which frequency bands to show based on VisualizationSide
        let total_bands = frequency_data.bands.len().min(32);
        let (bands_to_show, band_start_index) = match visualization_side {
            VisualizationSide::Left => (total_bands / 2, 0),
            VisualizationSide::Right => (total_bands / 2, total_bands / 2),
            VisualizationSide::Full => (total_bands, 0),
        };

        // Normalization factor for frequency amplitudes
        let normalization_factor = 1.0 / FREQUENCY_NORMALIZATION_MAX;
        let max_height = self.config.max_element_height(effective_bounds.height);

        // Prepare control points from frequency bands
        let mut control_points: Vec<(f32, f32)> = Vec::new();

        // Add virtual zero points at edges for smooth interpolation transitions
        // These create proper control points for the Catmull-Rom spline
        match visualization_side {
            VisualizationSide::Full => {
                // Add virtual zero point slightly before the start for smooth entry
                control_points.push((-0.1, 0.0));
            }
            VisualizationSide::Left => {
                // Add virtual zero point at left edge (outer edge) for smooth fade-in
                control_points.push((-0.1, 0.0));
            }
            VisualizationSide::Right => {
                // No zero point at start - the waveform should continue from left
            }
        }

        // Map each frequency band to a control point
        for display_band in 0..bands_to_show {
            let band_index = band_start_index + display_band;
            let amplitude = if band_index < frequency_data.bands.len() {
                frequency_data.bands[band_index] * normalization_factor
            } else {
                0.0
            };

            // Position along x-axis (normalized 0.0 to 1.0)
            let x_position = match visualization_side {
                VisualizationSide::Full => (display_band as f32 + 0.5) / bands_to_show as f32,
                VisualizationSide::Left | VisualizationSide::Right => {
                    // For side modes, spread evenly across full width
                    display_band as f32 / (bands_to_show - 1).max(1) as f32
                }
            };

            // Height from bottom (clamped to max)
            let height = (amplitude * max_height).min(max_height);

            control_points.push((x_position, height));
        }

        // Add virtual zero points at ending edges for smooth interpolation transitions
        match visualization_side {
            VisualizationSide::Full => {
                // Add virtual zero point slightly after the end for smooth exit
                control_points.push((1.1, 0.0));
            }
            VisualizationSide::Left => {
                // No zero point at end (inner edge) - maintains continuity with right side
            }
            VisualizationSide::Right => {
                // Add virtual zero point slightly after right edge for smooth fade-out
                control_points.push((1.1, 0.0));
            }
        }

        for _ in 0..(SMOOTHING_FACTOR as usize) {
            let mut smoothed_points = control_points.clone();
            for i in 1..control_points.len() - 1 {
                let prev_height = control_points[i - 1].1;
                let curr_height = control_points[i].1;
                let next_height = control_points[i + 1].1;
                smoothed_points[i].1 = prev_height * 0.25 + curr_height * 0.5 + next_height * 0.25;
            }
            control_points = smoothed_points;
        }

        // Calculate rendering points with higher density for smooth curve
        let render_points = effective_bounds.width as usize;

        let bottom_y = effective_bounds.y + effective_bounds.height;

        // Calculate wave curve points first
        let mut wave_points = Vec::new();

        // Determine the interpolation range based on control points
        let min_x = control_points
            .iter()
            .map(|(x, _)| *x)
            .fold(f32::INFINITY, f32::min);
        let max_x = control_points
            .iter()
            .map(|(x, _)| *x)
            .fold(f32::NEG_INFINITY, f32::max);

        // Draw the waveform using cubic interpolation between control points
        for i in 0..=render_points {
            let t = min_x + (i as f32 / render_points as f32) * (max_x - min_x);

            // Find the two control points we're between
            let mut prev_idx = 0;
            for j in 0..control_points.len() - 1 {
                if t >= control_points[j].0 && t <= control_points[j + 1].0 {
                    prev_idx = j;
                    break;
                }
            }

            let next_idx = (prev_idx + 1).min(control_points.len() - 1);

            // Get the four control points for Catmull-Rom interpolation
            let p0_idx = prev_idx.saturating_sub(1);
            let p1_idx = prev_idx;
            let p2_idx = next_idx;
            let p3_idx = (next_idx + 1).min(control_points.len() - 1);

            let p0 = control_points[p0_idx].1;
            let p1 = control_points[p1_idx].1;
            let p2 = control_points[p2_idx].1;
            let p3 = control_points[p3_idx].1;

            // Calculate local t between the two points
            #[allow(clippy::float_cmp)]
            let local_t = if control_points[p2_idx].0 == control_points[p1_idx].0 {
                0.0
            } else {
                (t - control_points[p1_idx].0)
                    / (control_points[p2_idx].0 - control_points[p1_idx].0)
            };

            // Catmull-Rom spline interpolation for smooth curves
            let t2 = local_t * local_t;
            let t3 = t2 * local_t;

            let height = 0.5
                * ((2.0 * p1)
                    + (-p0 + p2) * local_t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3);

            // Ensure height is non-negative and within bounds
            let clamped_height = height.max(0.0).min(effective_bounds.height);

            // Calculate x and y positions
            // Map t from the control point range back to the 0.0-1.0 drawable range
            #[allow(clippy::float_cmp)]
            let drawable_t = if max_x == min_x {
                0.0
            } else {
                (t - min_x) / (max_x - min_x)
            }
            .clamp(0.0, 1.0);

            let x = effective_bounds.x + drawable_t * effective_bounds.width;
            let y = bottom_y - clamped_height;

            wave_points.push(Point { x, y });
        }

        // Create the stroke path
        let mut stroke_path_builder = path::Builder::new();
        if let Some(first_point) = wave_points.first() {
            // Hack to get the stroke to be visible when all points have the same y-coordinate
            stroke_path_builder.line_to(Point {
                x: first_point.x,
                y: first_point.y - 0.000_001,
            });

            for point in wave_points.iter().skip(1) {
                stroke_path_builder.line_to(*point);
            }
        }

        // Create the fill path (curve + bottom area)
        let mut fill_path_builder = path::Builder::new();
        if let Some(first_point) = wave_points.first() {
            // Start from bottom-left
            fill_path_builder.move_to(Point {
                x: first_point.x,
                y: bottom_y,
            });

            // Draw to first wave point
            fill_path_builder.line_to(*first_point);

            // Follow the wave curve
            for point in wave_points.iter().skip(1) {
                fill_path_builder.line_to(*point);
            }

            // Complete the fill area by going to bottom-right and closing
            if let Some(last_point) = wave_points.last() {
                fill_path_builder.line_to(Point {
                    x: last_point.x,
                    y: bottom_y,
                });
            }
            fill_path_builder.close();
        }

        let base = color_config.get_color_with_theme(is_dark, cosmic_theme);

        let fill_path = fill_path_builder.build();
        let stroke_path = stroke_path_builder.build();

        // Draw the filled area with transparency
        frame.fill(
            &fill_path,
            Fill {
                style: stroke::Style::Solid(cosmic::iced::Color::from_rgba(
                    base.r,
                    base.g,
                    base.b,
                    base.a * FILL_OPACITY,
                )),
                ..Default::default()
            },
        );

        // Draw the wave curve outline
        frame.stroke(
            &stroke_path,
            stroke::Stroke {
                style: stroke::Style::Solid(base),
                width: STROKE_WIDTH,
                line_cap: cosmic::iced::widget::canvas::LineCap::Round,
                line_join: cosmic::iced::widget::canvas::LineJoin::Round,
                ..Default::default()
            },
        );
    }
}
