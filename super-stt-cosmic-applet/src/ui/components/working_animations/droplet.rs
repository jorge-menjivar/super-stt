// SPDX-License-Identifier: GPL-3.0-only
use std::f32::consts::PI;

use cosmic::{
    Renderer,
    iced::{
        Color, Point,
        widget::canvas::{Frame, path, stroke},
    },
};

use super::{WorkingAnimationRenderer, WorkingDrawContext, logical_span, smoothstep};

const PERIOD_MS: f32 = 1900.0;
const RAMP_MS: f32 = 260.0;

/// Vertical offset (px, upward positive) of the rippling line at distance `d`
/// (px) from the ripple origin. `half_width` is the distance from the origin
/// to the far edge of the *logical full* animation, so the logical full width
/// is `2 * half_width` (for a `Full` applet that's the applet width; for a side
/// applet it's twice the applet width, with the origin at the inner edge).
/// Pure + deterministic so it can be unit-tested without a renderer.
#[allow(clippy::many_single_char_names)] // math variables: d, h, c, k are standard notation
pub(crate) fn droplet_offset(d: f32, half_width: f32, elapsed_ms: f32, h: f32) -> f32 {
    if half_width <= 0.0 {
        return 0.0;
    }
    let wfull = 2.0 * half_width;
    let c = wfull / 1700.0;
    let k = 3.0 * 4.0 * PI / wfull;
    let omega = c * k;
    let a0 = h * 0.38;
    let spatial = 2.2 / wfull;
    let tdecay = 0.0026;
    let drop = |age: f32| -> f32 {
        let la = age - d / c;
        if la <= 0.0 {
            return 0.0;
        }
        a0 * (-d * spatial).exp()
            * (-la * tdecay).exp()
            * smoothstep(0.0, RAMP_MS, la)
            * (omega * la).sin()
    };
    let age_now = elapsed_ms % PERIOD_MS;
    drop(age_now) + drop(age_now + PERIOD_MS)
}

pub struct DropletAnimation;

impl WorkingAnimationRenderer for DropletAnimation {
    #[allow(clippy::many_single_char_names)] // math variables: x, y, w, h are standard notation
    fn draw(&self, frame: &mut Frame<Renderer>, ctx: &WorkingDrawContext) {
        let WorkingDrawContext {
            bounds,
            elapsed_ms,
            color_config,
            is_dark,
            cosmic_theme,
            side,
        } = *ctx;
        let (w, h) = (bounds.width, bounds.height);
        let mid_y = bounds.y + h / 2.0;
        let base = color_config.get_color_with_theme(is_dark, cosmic_theme);

        // Split the ripple by side: the origin sits at the logical centre
        // (`half_width` from each edge of the logical full animation). For a
        // side applet that centre lands on its inner edge, so Left + Right form
        // one ripple seamed at the panel middle.
        let (wlog, x_lo) = logical_span(side, w);
        let half_width = wlog / 2.0;

        let mut builder = path::Builder::new();
        let mut lx = 0.0;
        let mut first = true;
        while lx <= w {
            let d = (x_lo + lx - half_width).abs();
            let y = droplet_offset(d, half_width, elapsed_ms, h);
            let p = Point::new(bounds.x + lx, mid_y - y);
            if first {
                builder.move_to(p);
                first = false;
            } else {
                builder.line_to(p);
            }
            lx += 2.0;
        }
        let line = builder.build();

        // Glow approximation: a wide, faint under-stroke beneath the line.
        frame.stroke(
            &line,
            stroke::Stroke {
                style: stroke::Style::Solid(Color::from_rgba(
                    base.r,
                    base.g,
                    base.b,
                    base.a * 0.25,
                )),
                width: 5.0,
                line_cap: cosmic::iced::widget::canvas::LineCap::Round,
                line_join: cosmic::iced::widget::canvas::LineJoin::Round,
                ..Default::default()
            },
        );
        frame.stroke(
            &line,
            stroke::Stroke {
                style: stroke::Style::Solid(base),
                width: 2.4,
                line_cap: cosmic::iced::widget::canvas::LineCap::Round,
                line_join: cosmic::iced::widget::canvas::LineJoin::Round,
                ..Default::default()
            },
        );

        // Impact flash at the ripple origin (the logical centre) for the first
        // ~180ms of each droplet. On a side applet that origin maps to the inner
        // edge, so the flash appears at the seam.
        let impact = (1.0 - (elapsed_ms % PERIOD_MS) / 180.0).max(0.0);
        if impact > 0.0 {
            let origin_x = bounds.x + (half_width - x_lo);
            let dot = path::Path::circle(Point::new(origin_x, mid_y), 2.4 + 3.0 * impact);
            frame.fill(
                &dot,
                Color::from_rgba(base.r, base.g, base.b, base.a * 0.8 * impact),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(
        clippy::many_single_char_names,
        reason = "geometry: width/height/half-width/distance/offset"
    )]
    fn near_rest_at_start() {
        // Full applet: half_width = w/2, origin (d=0) at the centre.
        let (w, h) = (240.0_f32, 60.0_f32);
        let hw = w / 2.0;
        let mut x = 0.0;
        while x <= w {
            let d = (x - hw).abs();
            let y = droplet_offset(d, hw, 0.0, h);
            assert!(y.abs() <= 0.02 * h, "offset {y} at x={x} exceeds 2% of H");
            x += 2.0;
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)] // loop counter
    fn moves_after_a_drop() {
        let (w, h) = (240.0_f32, 60.0_f32);
        let hw = w / 2.0;
        let mut max = 0.0_f32;
        for i in 0..=120 {
            let d = (i as f32 * 2.0 - hw).abs();
            max = max.max(droplet_offset(d, hw, 400.0, h).abs());
        }
        assert!(
            max > 0.05 * h,
            "expected visible motion mid-cycle, got {max}"
        );
    }
}
