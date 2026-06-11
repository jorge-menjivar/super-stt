// SPDX-License-Identifier: GPL-3.0-only
use cosmic::{
    Renderer,
    iced::{
        Color, Point,
        widget::canvas::{Frame, path},
    },
};

use super::{WorkingAnimationRenderer, WorkingDrawContext};

/// Three dots pulsing in sequence — a compact "working" indicator.
///
/// Unlike the wave animations, this renderer **ignores `side`**: it renders the
/// dots centred in full on every applet (exempt from the left/right split), so
/// the loader reads the same on a full applet or on either side applet.
pub struct DotsAnimation;

impl WorkingAnimationRenderer for DotsAnimation {
    #[allow(clippy::cast_precision_loss)] // `i` is a 0..3 dot index
    #[allow(clippy::many_single_char_names)] // math variables: w, h, r are standard notation
    fn draw(&self, frame: &mut Frame<Renderer>, ctx: &WorkingDrawContext) {
        // `side` is intentionally unused — this loader is exempt from the split.
        let WorkingDrawContext {
            bounds,
            elapsed_ms,
            color_config,
            is_dark,
            cosmic_theme,
            ..
        } = *ctx;
        let (w, h) = (bounds.width, bounds.height);
        if w <= 0.0 {
            return;
        }
        let base = color_config.get_color_with_theme(is_dark, cosmic_theme);
        let cy = bounds.y + h / 2.0;
        let gap = (w * 0.085).min(28.0);
        let r = (h * 0.18).min(7.0);
        // Centre the three-dot group on the applet centre (dots at -gap, 0, +gap).
        let cx = bounds.x + w / 2.0 - gap;

        for i in 0..3 {
            // Each dot pulses, offset in phase so they ripple left-to-right.
            let s = 0.45f32.mul_add((elapsed_ms * 0.004 - i as f32 * 0.6).sin(), 0.55);
            let alpha = 0.7f32.mul_add(s, 0.3);
            let radius = r * 0.5f32.mul_add(s, 0.7);
            let x = cx + i as f32 * gap;

            // Soft glow under-dot (iced has no shadow-blur).
            let glow = path::Path::circle(Point::new(x, cy), radius + 2.0);
            frame.fill(
                &glow,
                Color::from_rgba(base.r, base.g, base.b, base.a * alpha * 0.3),
            );
            let dot = path::Path::circle(Point::new(x, cy), radius);
            frame.fill(
                &dot,
                Color::from_rgba(base.r, base.g, base.b, base.a * alpha),
            );
        }
    }
}
