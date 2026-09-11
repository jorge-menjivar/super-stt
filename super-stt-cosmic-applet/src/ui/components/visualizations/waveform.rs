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
    // No control points, no curve. The segment search below indexes
    // `control_points.len() - 1`, which underflows on an empty slice.
    if control_points.is_empty() {
        return Vec::new();
    }

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

#[cfg(test)]
mod waveform_tests {
    //! The waveform is the one visualization built from a spline rather than
    //! from rectangles, so its geometry has failure modes the bar renderers
    //! don't: a Catmull-Rom curve overshoots around a spike, and the left and
    //! right applets have to meet at the seam without a step. These pin the
    //! pure math behind `draw`, which needs a live renderer and can't be
    //! exercised here.
    use super::*;
    use crate::util::usize_to_f32;
    use cosmic::iced::widget::canvas::path::lyon_path::PathEvent;

    fn bounds(width: f32, height: f32) -> Rectangle {
        Rectangle {
            x: 0.0,
            y: 0.0,
            width,
            height,
        }
    }

    fn frequency_data(bands: Vec<f32>) -> FrequencyData {
        FrequencyData {
            bands,
            ..FrequencyData::default()
        }
    }

    fn heights(points: &[(f32, f32)]) -> Vec<f32> {
        points.iter().map(|(_, height)| *height).collect()
    }

    #[test]
    fn the_two_halves_meet_at_the_seam() {
        // A Left applet and a Right applet draw one wave across a gap. Left
        // pads only its outer edge and runs its last band up to x = 1.0;
        // Right starts at x = 0.0 on its first band and pads only its own
        // outer edge. Pad the inner edges too and the wave would dive to the
        // baseline in the middle of the panel.
        let viz = WaveformVisualization::default();
        let data = frequency_data(vec![FREQUENCY_NORMALIZATION_MAX; 4]);
        let bounds = bounds(120.0, 40.0);

        let full = viz.build_control_points(&data, &VisualizationSide::Full, bounds);
        assert_eq!(full.first().unwrap().0, -0.1);
        assert_eq!(full.last().unwrap().0, 1.1);

        let left = viz.build_control_points(&data, &VisualizationSide::Left, bounds);
        assert_eq!(left.first().unwrap().0, -0.1);
        assert_eq!(left.last().unwrap().0, 1.0);
        assert!(
            left.last().unwrap().1 > 0.0,
            "left dropped to the baseline at the seam"
        );

        let right = viz.build_control_points(&data, &VisualizationSide::Right, bounds);
        assert_eq!(right.first().unwrap().0, 0.0);
        assert!(
            right.first().unwrap().1 > 0.0,
            "right dropped to the baseline at the seam"
        );
        assert_eq!(right.last().unwrap().0, 1.1);
    }

    #[test]
    fn amplitudes_are_normalized_and_capped_at_the_drawable_height() {
        let viz = WaveformVisualization::default();
        let bounds = bounds(120.0, 40.0);
        let data = frequency_data(vec![
            FREQUENCY_NORMALIZATION_MAX,
            FREQUENCY_NORMALIZATION_MAX / 2.0,
            FREQUENCY_NORMALIZATION_MAX * 100.0,
            0.0,
        ]);

        let points = viz.build_control_points(&data, &VisualizationSide::Full, bounds);

        // Index 0 is the leading virtual zero, so the bands start at 1.
        assert!((points[1].1 - 40.0).abs() < 1e-3);
        assert!((points[2].1 - 20.0).abs() < 1e-3);
        assert!(
            (points[3].1 - 40.0).abs() < 1e-3,
            "a band past the normalization max has to cap, not run off the panel",
        );
        assert_eq!(points[4].1, 0.0);
    }

    #[test]
    fn control_points_advance_left_to_right() {
        let viz = WaveformVisualization::default();
        let data = frequency_data(vec![1.0; 8]);

        let points = viz.build_control_points(&data, &VisualizationSide::Full, bounds(120.0, 40.0));

        assert!(
            points.windows(2).all(|pair| pair[1].0 > pair[0].0),
            "the spline needs strictly increasing x to find its segments",
        );
    }

    #[test]
    fn smoothing_keeps_the_ends_pinned() {
        // The virtual zeros are what make the wave fade in and out at the
        // panel edges. Smoothing must not lift them off the baseline.
        let mut points = vec![
            (-0.1, 0.0),
            (0.25, 30.0),
            (0.5, 12.0),
            (0.75, 4.0),
            (1.1, 0.0),
        ];

        smooth_control_points(&mut points);

        assert_eq!(points.first().unwrap().1, 0.0);
        assert_eq!(points.last().unwrap().1, 0.0);
    }

    #[test]
    fn smoothing_spreads_a_spike_without_overshooting_it() {
        let original = vec![
            (0.0, 0.0),
            (0.25, 0.0),
            (0.5, 30.0),
            (0.75, 0.0),
            (1.0, 0.0),
        ];
        let mut points = original.clone();

        smooth_control_points(&mut points);

        assert!(points[2].1 < 30.0, "the peak has to come down");
        assert!(
            points[1].1 > 0.0 && points[3].1 > 0.0,
            "and its neighbours have to come up",
        );
        // Each pass is a convex combination, so no height can leave the range
        // it started in — that is what keeps a smoothed wave inside the panel.
        let ceiling = heights(&original).into_iter().fold(f32::MIN, f32::max);
        assert!(heights(&points).iter().all(|h| (0.0..=ceiling).contains(h)));
    }

    #[test]
    fn smoothing_leaves_a_flat_wave_alone() {
        let mut points: Vec<(f32, f32)> = (0..6).map(|i| (usize_to_f32(i), 5.0)).collect();

        smooth_control_points(&mut points);

        assert!(heights(&points).iter().all(|h| (h - 5.0).abs() < 1e-6));
    }

    #[test]
    fn smoothing_survives_a_wave_too_short_to_smooth() {
        for len in 0..3 {
            let mut points: Vec<(f32, f32)> = (0..len).map(|i| (usize_to_f32(i), 10.0)).collect();

            smooth_control_points(&mut points);

            assert_eq!(points.len(), len);
        }
    }

    #[test]
    fn the_curve_is_sampled_once_per_horizontal_pixel() {
        let viz = WaveformVisualization::default();
        let data = frequency_data(vec![1.0; 8]);
        let bounds = Rectangle {
            x: 10.0,
            y: 5.0,
            width: 120.0,
            height: 40.0,
        };
        let control = viz.build_control_points(&data, &VisualizationSide::Full, bounds);

        let wave = compute_wave_points(&control, bounds);

        assert_eq!(wave.len(), 121);
        assert!((wave.first().unwrap().x - 10.0).abs() < 1e-3);
        assert!((wave.last().unwrap().x - 130.0).abs() < 1e-3);
        assert!(
            wave.windows(2).all(|pair| pair[1].x >= pair[0].x),
            "sample x must never go backwards",
        );
    }

    #[test]
    fn a_spline_overshoot_is_clamped_inside_the_panel() {
        // Catmull-Rom overshoots on both sides of a sharp step. Unclamped,
        // the curve would be drawn above the applet and below its baseline.
        let bounds = bounds(60.0, 40.0);
        let control = vec![(-0.1, 0.0), (0.2, 0.0), (0.4, 40.0), (0.6, 0.0), (1.1, 0.0)];

        let wave = compute_wave_points(&control, bounds);

        let bottom_y = bounds.y + bounds.height;
        for point in &wave {
            assert!(
                point.y >= bounds.y - 1e-3 && point.y <= bottom_y + 1e-3,
                "sample at x={} left the panel at y={}",
                point.x,
                point.y,
            );
        }
    }

    #[test]
    fn a_silent_wave_sits_flat_on_the_baseline() {
        let bounds = bounds(60.0, 40.0);
        let control = vec![(-0.1, 0.0), (0.5, 0.0), (1.1, 0.0)];

        let wave = compute_wave_points(&control, bounds);

        let bottom_y = bounds.y + bounds.height;
        assert!(wave.iter().all(|point| (point.y - bottom_y).abs() < 1e-4));
    }

    #[test]
    fn no_control_points_yields_no_samples() {
        // `build_control_points` always emits at least one virtual pad, so
        // this is unreachable today — but the sampler indexes `len() - 1`,
        // which underflows the moment that stops being true.
        assert!(compute_wave_points(&[], bounds(60.0, 40.0)).is_empty());
    }

    #[test]
    fn coincident_control_points_do_not_produce_nan() {
        // Both divisions in the sampler are guarded against a zero span; an
        // unguarded one would put NaN into the path and blank the applet.
        let wave = compute_wave_points(&[(0.5, 0.0), (0.5, 10.0), (0.5, 0.0)], bounds(60.0, 40.0));

        assert!(
            wave.iter()
                .all(|point| point.x.is_finite() && point.y.is_finite())
        );
    }

    #[test]
    fn the_fill_path_closes_along_the_baseline() {
        let bottom_y = 40.0;
        let wave = vec![
            Point { x: 0.0, y: 30.0 },
            Point { x: 1.0, y: 20.0 },
            Point { x: 2.0, y: 25.0 },
        ];

        let (stroke, fill) = build_paths(&wave, bottom_y);

        let fill_events: Vec<PathEvent> = fill.raw().iter().collect();
        let Some(PathEvent::Begin { at }) = fill_events.first() else {
            panic!("the fill path never began");
        };
        assert!(
            (at.y - bottom_y).abs() < 1e-4,
            "the filled area has to start on the baseline, not on the curve",
        );
        assert!(
            fill_events
                .iter()
                .any(|event| matches!(event, PathEvent::End { close: true, .. })),
            "an unclosed fill leaves the area under the curve unpainted",
        );

        assert!(
            stroke
                .raw()
                .iter()
                .any(|event| matches!(event, PathEvent::End { close: false, .. })),
            "the outline is an open curve, not a closed shape",
        );
    }

    #[test]
    fn the_stroke_path_follows_the_wave() {
        let wave = vec![
            Point { x: 0.0, y: 30.0 },
            Point { x: 1.0, y: 20.0 },
            Point { x: 2.0, y: 25.0 },
        ];

        let (stroke, _) = build_paths(&wave, 40.0);

        let lines = stroke
            .raw()
            .iter()
            .filter(|event| matches!(event, PathEvent::Line { .. }))
            .count();
        assert_eq!(
            lines,
            wave.len() - 1,
            "one segment between each pair of samples"
        );
    }

    #[test]
    fn no_wave_points_means_no_paths() {
        let (stroke, fill) = build_paths(&[], 40.0);

        assert_eq!(stroke.raw().iter().count(), 0);
        assert_eq!(fill.raw().iter().count(), 0);
    }
}
