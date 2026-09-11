// SPDX-License-Identifier: GPL-3.0-only
use cosmic::{
    Element, Renderer, Theme,
    iced::{
        core::{Rectangle, mouse},
        widget::{
            Canvas,
            canvas::{Frame, Geometry, Program},
        },
    },
};

use crate::app::Message;
use crate::models::theme::{VisualizationColorConfig, VisualizationSide, WorkingAnimationTheme};
use crate::ui::components::working_animations::{self, WorkingDrawContext};

/// Time-driven "working" animation canvas, shown during the transcribing
/// (`Processing`) phase. Analogous to `VisualizationComponent` but driven by
/// elapsed time instead of audio data.
#[derive(Debug, Clone)]
pub struct WorkingAnimationComponent {
    theme: WorkingAnimationTheme,
    colors: VisualizationColorConfig,
    /// Which side this applet renders (fixed per binary variant). Wave-style
    /// animations split on it so the side applets seam at the middle; compact
    /// indicators may ignore it and render in full on every side.
    side: VisualizationSide,
    elapsed_ms: f32,
}

impl WorkingAnimationComponent {
    pub fn new(
        theme: WorkingAnimationTheme,
        side: VisualizationSide,
        colors: VisualizationColorConfig,
    ) -> Self {
        Self {
            theme,
            colors,
            side,
            elapsed_ms: 0.0,
        }
    }

    pub fn set_elapsed(&mut self, elapsed_ms: f32) {
        self.elapsed_ms = elapsed_ms;
    }

    pub fn reset(&mut self) {
        self.elapsed_ms = 0.0;
    }

    /// Switch the working animation theme.
    pub fn update_theme(&mut self, theme: WorkingAnimationTheme) {
        self.theme = theme;
    }

    pub fn update_colors(&mut self, colors: VisualizationColorConfig) {
        self.colors = colors;
    }
}

impl<'a> From<WorkingAnimationComponent> for Element<'a, Message> {
    fn from(component: WorkingAnimationComponent) -> Element<'a, Message> {
        Canvas::new(component)
            .width(cosmic::iced::Length::Fill)
            .height(cosmic::iced::Length::Fill)
            .into()
    }
}

impl Program<Message, Theme, Renderer> for WorkingAnimationComponent {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        frame.fill_rectangle(
            cosmic::iced::Point::ORIGIN,
            bounds.size(),
            cosmic::iced::Color::TRANSPARENT,
        );
        let ctx = WorkingDrawContext {
            bounds,
            elapsed_ms: self.elapsed_ms,
            color_config: &self.colors,
            is_dark: theme.cosmic().is_dark,
            cosmic_theme: theme.cosmic(),
            side: &self.side,
        };
        working_animations::draw(self.theme, &mut frame, &ctx);
        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod working_animation_component_tests {
    use super::*;

    fn component() -> WorkingAnimationComponent {
        WorkingAnimationComponent::new(
            WorkingAnimationTheme::Droplet,
            VisualizationSide::Full,
            VisualizationColorConfig::default(),
        )
    }

    #[test]
    fn a_new_animation_starts_at_its_first_frame() {
        assert_eq!(component().elapsed_ms, 0.0);
    }

    #[test]
    fn resetting_rewinds_to_the_first_frame() {
        // The applet resets the component whenever the transcribing clock
        // starts, so a second recording opens on frame zero instead of
        // wherever the previous one stopped.
        let mut component = component();
        component.set_elapsed(4_200.0);

        component.reset();

        assert_eq!(component.elapsed_ms, 0.0);
    }

    #[test]
    fn switching_theme_keeps_the_clock_running() {
        // Picking a different animation mid-cycle must not restart it: the
        // clock lives in the applet, and the component only follows it.
        let mut component = component();
        component.set_elapsed(1_500.0);

        component.update_theme(WorkingAnimationTheme::Comet);

        assert_eq!(component.elapsed_ms, 1_500.0);
    }
}
