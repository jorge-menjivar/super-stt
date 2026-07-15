// SPDX-License-Identifier: GPL-3.0-only
use crate::config::FREQUENCY_NORMALIZATION_MAX;
use crate::models::theme::VisualizationSide;
use crate::ui::components::visualizations::{
    DrawContext, VisualizationConfig, VisualizationRenderer, visible_band_range,
};
use crate::util::{f32_to_usize, usize_to_f32};
use cosmic::iced::{
    Padding, Point, Radius,
    core::Rectangle,
    widget::canvas::{Fill, Frame, path, stroke},
};
use super_stt_shared::FrequencyData;

/// Bottom-aligned frequency waveform: frequency bands rendered as a
/// smooth continuous wave rising from the bottom edge.
pub struct WaveformVisualization {
    config: VisualizationConfig,
}

const SMOOTHING_PASSES: usize = 4;
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

        let mut control_points = self.build_control_points(frequency_data, side, effective_bounds);
        smooth_control_points(&mut control_points);

        let bottom_y = effective_bounds.y + effective_bounds.height;
        let wave_points = compute_wave_points(&control_points, effective_bounds);
        let (stroke_path, fill_path) = build_paths(&wave_points, bottom_y);

        let base = color_config.get_color_with_theme(is_dark, cosmic_theme);

        // Filled area under the curve, with transparency.
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

        // Curve outline on top.
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

impl WaveformVisualization {
    /// Map the visible frequency bands to `(x, height)` control points,
    /// padding the ends with virtual zero points so the spline enters and
    /// exits smoothly.
    fn build_control_points(
        &self,
        frequency_data: &FrequencyData,
        side: &VisualizationSide,
        effective_bounds: Rectangle,
    ) -> Vec<(f32, f32)> {
        let (bands_to_show, band_start_index) =
            visible_band_range(side, frequency_data.bands.len());

        let normalization_factor = 1.0 / FREQUENCY_NORMALIZATION_MAX;
        let max_height = self.config.max_element_height(effective_bounds.height);

        let mut control_points: Vec<(f32, f32)> = Vec::new();

        // Leading virtual zero point for a smooth fade-in (the Right side
        // continues from the left half, so it gets none).
        if !matches!(side, VisualizationSide::Right) {
            control_points.push((-0.1, 0.0));
        }

        for display_band in 0..bands_to_show {
            let band_index = band_start_index + display_band;
            let amplitude = if band_index < frequency_data.bands.len() {
                frequency_data.bands[band_index] * normalization_factor
            } else {
                0.0
            };

            // Normalized x position (0.0..1.0).
            let x_position = match side {
                VisualizationSide::Full => {
                    (usize_to_f32(display_band) + 0.5) / usize_to_f32(bands_to_show)
                }
                VisualizationSide::Left | VisualizationSide::Right => {
                    usize_to_f32(display_band) / usize_to_f32((bands_to_show - 1).max(1))
                }
            };

            let height = (amplitude * max_height).min(max_height);
            control_points.push((x_position, height));
        }

        // Trailing virtual zero point for a smooth fade-out (the Left side
        // keeps continuity with the right half, so it gets none).
        if !matches!(side, VisualizationSide::Left) {
            control_points.push((1.1, 0.0));
        }

        control_points
    }
}

/// Smooth control-point heights with repeated 3-tap averaging passes.
fn smooth_control_points(points: &mut Vec<(f32, f32)>) {
    for _ in 0..SMOOTHING_PASSES {
        let mut smoothed = points.clone();
        for i in 1..points.len().saturating_sub(1) {
            let prev = points[i - 1].1;
            let curr = points[i].1;
            let next = points[i + 1].1;
            smoothed[i].1 = prev * 0.25 + curr * 0.5 + next * 0.25;
        }
        *points = smoothed;
    }
}

/// Sample the Catmull-Rom spline through `control_points` at one point per
/// horizontal pixel, returning canvas-space points along the curve.
fn compute_wave_points(control_points: &[(f32, f32)], effective_bounds: Rectangle) -> Vec<Point> {
    let render_points = f32_to_usize(effective_bounds.width);
    let bottom_y = effective_bounds.y + effective_bounds.height;

    let min_x = control_points
        .iter()
        .map(|(x, _)| *x)
        .fold(f32::INFINITY, f32::min);
    let max_x = control_points
        .iter()
        .map(|(x, _)| *x)
        .fold(f32::NEG_INFINITY, f32::max);
    let span = max_x - min_x;

    let mut wave_points = Vec::new();
    for i in 0..=render_points {
        let t = min_x + (usize_to_f32(i) / usize_to_f32(render_points)) * span;

        // Find the segment containing `t`.
        let mut prev_idx = 0;
        for j in 0..control_points.len().saturating_sub(1) {
            if t >= control_points[j].0 && t <= control_points[j + 1].0 {
                prev_idx = j;
                break;
            }
        }
        let next_idx = (prev_idx + 1).min(control_points.len() - 1);

        // Four points for Catmull-Rom interpolation.
        let p0 = control_points[prev_idx.saturating_sub(1)].1;
        let p1 = control_points[prev_idx].1;
        let p2 = control_points[next_idx].1;
        let p3 = control_points[(next_idx + 1).min(control_points.len() - 1)].1;

        // Local parameter within the segment; degenerate segments map to 0.
        let segment = control_points[next_idx].0 - control_points[prev_idx].0;
        let local_t = if segment.abs() < f32::EPSILON {
            0.0
        } else {
            (t - control_points[prev_idx].0) / segment
        };

        let t2 = local_t * local_t;
        let t3 = t2 * local_t;
        let height = 0.5
            * ((2.0 * p1)
                + (-p0 + p2) * local_t
                + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3);
        let clamped_height = height.max(0.0).min(effective_bounds.height);

        // Map `t` back into the 0.0..1.0 drawable range.
        let drawable_t = if span.abs() < f32::EPSILON {
            0.0
        } else {
            (t - min_x) / span
        }
        .clamp(0.0, 1.0);

        wave_points.push(Point {
            x: effective_bounds.x + drawable_t * effective_bounds.width,
            y: bottom_y - clamped_height,
        });
    }

    wave_points
}

/// Build the stroke (curve outline) and fill (curve plus baseline) paths
/// from the sampled wave points.
fn build_paths(wave_points: &[Point], bottom_y: f32) -> (path::Path, path::Path) {
    let mut stroke_builder = path::Builder::new();
    if let Some(first) = wave_points.first() {
        // Nudge the first point so a perfectly flat wave still strokes.
        stroke_builder.line_to(Point {
            x: first.x,
            y: first.y - 0.000_001,
        });
        for point in wave_points.iter().skip(1) {
            stroke_builder.line_to(*point);
        }
    }

    let mut fill_builder = path::Builder::new();
    if let Some(first) = wave_points.first() {
        fill_builder.move_to(Point {
            x: first.x,
            y: bottom_y,
        });
        fill_builder.line_to(*first);
        for point in wave_points.iter().skip(1) {
            fill_builder.line_to(*point);
        }
        if let Some(last) = wave_points.last() {
            fill_builder.line_to(Point {
                x: last.x,
                y: bottom_y,
            });
        }
        fill_builder.close();
    }

    (stroke_builder.build(), fill_builder.build())
}
