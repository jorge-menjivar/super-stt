// SPDX-License-Identifier: GPL-3.0-only
use std::f32::consts::PI;

use cosmic::{
    Renderer,
    iced::{
        Color, Point,
        widget::canvas::{Frame, path, stroke},
    },
};

use super::{WorkingAnimationRenderer, WorkingDrawContext, logical_span};

pub struct CometAnimation;

impl WorkingAnimationRenderer for CometAnimation {
    #[allow(clippy::cast_precision_loss)] // trail index is a tiny loop counter
    #[allow(clippy::many_single_char_names)] // math variables: w, h, k, a are standard notation
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
        if w <= 0.0 {
            return;
        }
        let mid_y = bounds.y + h / 2.0;
        let amp = h * 0.30;
        let base = color_config.get_color_with_theme(is_dark, cosmic_theme);

        // Split by side: the comet path/head live in a logical full-width space;
        // this applet renders the `[x_lo, x_lo + w)` slice of it, so the head
        // travels continuously from the Left applet into the Right one.
        let (wlog, x_lo) = logical_span(side, w);
        let k = 2.0 * PI * 1.8 / wlog;
        // `lx` is a logical x-coordinate; `lx - x_lo` maps it to this canvas.
        let path_y = |lx: f32| mid_y + amp * (lx * k).sin();

        // Faint guide line over this applet's slice.
        let mut gb = path::Builder::new();
        let mut local = 0.0;
        let mut first = true;
        while local <= w {
            let p = Point::new(bounds.x + local, path_y(x_lo + local));
            if first {
                gb.move_to(p);
                first = false;
            } else {
                gb.line_to(p);
            }
            local += 3.0;
        }
        frame.stroke(
            &gb.build(),
            stroke::Stroke {
                style: stroke::Style::Solid(Color::from_rgba(
                    base.r,
                    base.g,
                    base.b,
                    base.a * 0.18,
                )),
                width: 1.4,
                ..Default::default()
            },
        );

        // Comet head + fading trail, positioned in logical space and clipped to
        // this applet's slice.
        let trail: i32 = 44;
        let head = (elapsed_ms * 0.11) % (wlog + 40.0) - 20.0;
        for i in (0..=trail).rev() {
            let lx = head - (i as f32) * 3.0;
            if lx < x_lo || lx > x_lo + w {
                continue;
            }
            let sx = bounds.x + (lx - x_lo);
            let yy = path_y(lx);
            let a = 1.0 - (i as f32) / (trail as f32);
            if i == 0 {
                let glow = path::Path::circle(Point::new(sx, yy), 7.0);
                frame.fill(
                    &glow,
                    Color::from_rgba(base.r, base.g, base.b, base.a * 0.3),
                );
            }
            let r = if i == 0 { 4.0 } else { 2.0 * a + 0.6 };
            let dot = path::Path::circle(Point::new(sx, yy), r);
            frame.fill(
                &dot,
                Color::from_rgba(base.r, base.g, base.b, base.a * a * a * 0.9),
            );
        }
    }
}
