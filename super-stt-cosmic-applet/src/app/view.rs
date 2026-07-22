// SPDX-License-Identifier: GPL-3.0-only
use std::rc::Rc;

use cosmic::{
    Element,
    iced::{Alignment, Length, Size, widget as iced_widget},
    theme::{self, Button},
    widget::{self, button, container, layer_container, mouse_area},
};

use super::SuperSttApplet;
use crate::app::Message;
use crate::models::state::{DaemonConnectionState, RecordingState};
use crate::ui::views::{PopupContentParams, create_popup_content};
use crate::util::u32_to_f32;

// Cache icon bytes to avoid allocation on every render.
static NORMAL_ICON: &[u8] = include_bytes!("../../resources/assets/super-stt-icon.svg");
static TRANSPARENT_ICON: &[u8] = include_bytes!("../../resources/assets/transparent-icon.svg");
static ERROR_ICON: &[u8] = include_bytes!("../../resources/assets/error-icon.svg");

/// Visualization height in pixels.
const VISUALIZATION_HEIGHT: f32 = 100.0;

impl SuperSttApplet {
    pub(super) fn view_applet(&self) -> Element<'_, Message> {
        // Show visualizations only when the daemon is actively recording
        // and the user has visualizations enabled.
        let should_show_visualizations = matches!(self.recording_state, RecordingState::Recording)
            && self.config.ui.show_visualization;
        let should_show_working = matches!(self.recording_state, RecordingState::Processing)
            && self.config.ui.show_visualization;

        let visualization_size = self.visualization_size();

        if self.daemon_state == DaemonConnectionState::Connected && should_show_visualizations {
            let visualization_element =
                container(mouse_area(self.visualization.clone()).on_press(Message::TogglePopup))
                    .width(Length::Fixed(visualization_size.width))
                    .height(Length::Fixed(visualization_size.height));

            self.core
                .applet
                .autosize_window(visualization_element)
                .into()
        } else if self.daemon_state == DaemonConnectionState::Connected && should_show_working {
            let working_element = container(
                mouse_area(self.working_animation.clone()).on_press(Message::TogglePopup),
            )
            .width(Length::Fixed(visualization_size.width))
            .height(Length::Fixed(visualization_size.height));

            self.core.applet.autosize_window(working_element).into()
        } else {
            // The error glyph is monochrome line art and must follow the panel
            // theme to stay legible; the normal logo is full-color artwork that
            // a theme tint would flatten into a silhouette.
            let (icon_bytes, symbolic) = if !(self.daemon_state == DaemonConnectionState::Connected
                || self.daemon_state == DaemonConnectionState::Connecting)
            {
                (ERROR_ICON, true)
            } else if self.config.ui.show_icon {
                (NORMAL_ICON, false)
            } else {
                (TRANSPARENT_ICON, false)
            };

            let (applet_padding, _) = self.core.applet.suggested_padding(false);

            let icon_alignment = self.config.ui.icon_alignment.to_alignment();

            let icon_button = transparent_icon_button(
                icon_bytes,
                symbolic,
                visualization_size,
                applet_padding,
                icon_alignment,
            );

            self.core.applet.autosize_window(icon_button).into()
        }
    }

    pub(super) fn view_popup(&self) -> Element<'_, Message> {
        let content = create_popup_content(&PopupContentParams {
            daemon_state: &self.daemon_state,
            is_open: &self.is_open,
            config: &self.config,
            icon_alignment_model: &self.icon_alignment_model,
            theme_selector_model: &self.theme_selector_model,
            selected_theme_for_config: self.selected_theme_for_config,
        });

        self.core.applet.popup_container(content).into()
    }

    /// Compute the applet's window size from the panel orientation and
    /// user configuration. When visualizations are disabled the applet
    /// shrinks to a compact icon-only square.
    fn visualization_size(&self) -> Size {
        let (suggested_width, suggested_height) = self.core.applet.suggested_window_size();
        let (_, suggested_padding_h) = self.core.applet.suggested_padding(false);
        let padding = f32::from(suggested_padding_h);
        let horizontal = self.core.applet.is_horizontal();

        if self.config.ui.show_visualization {
            let configured_width = u32_to_f32(self.config.ui.applet_width);
            if horizontal {
                // Constrain by height but respect the user's width preference.
                let available_height = u32_to_f32(suggested_height.get()) - (padding * 2.0);
                let constrained_height = available_height.min(VISUALIZATION_HEIGHT + 8.0);
                let constrained_width = configured_width.min(available_height * 8.0).max(60.0);
                Size::new(constrained_width, constrained_height)
            } else {
                let available_width = u32_to_f32(suggested_width.get()) - (padding * 2.0);
                let constrained_width = configured_width.min(available_width * 2.0).max(60.0);
                Size::new(constrained_width, VISUALIZATION_HEIGHT + 8.0)
            }
        } else {
            let icon_size = if horizontal {
                (u32_to_f32(suggested_height.get()) - (padding * 2.0)).clamp(24.0, 48.0)
            } else {
                (u32_to_f32(suggested_width.get()) - (padding * 2.0)).clamp(24.0, 48.0)
            };
            Size::new(icon_size, icon_size)
        }
    }
}

/// Build the panel button wrapping `icon_bytes`.
///
/// `symbolic` selects how the SVG is colored. When set, the icon is recolored
/// wholesale to the panel's on-background color, which suits monochrome line
/// art. When unset, the SVG renders in its own colors: an explicit
/// `svg::Style::color` is applied over the entire image regardless of the
/// handle's symbolic flag, so full-color artwork must not be given a class at
/// all or it collapses into a flat silhouette.
fn transparent_icon_button<'a>(
    icon_bytes: &'static [u8],
    symbolic: bool,
    visualization_size: Size,
    applet_padding: u16,
    alignment: Alignment,
) -> cosmic::widget::Button<'a, Message> {
    let icon_size =
        (visualization_size.height.min(visualization_size.width) * 0.6).clamp(16.0, 32.0);

    let mut icon = widget::icon(widget::icon::from_svg_bytes(icon_bytes))
        .width(Length::Fixed(icon_size))
        .height(Length::Fixed(icon_size));

    if symbolic {
        icon = icon.class(theme::Svg::Custom(Rc::new(|theme| {
            iced_widget::svg::Style {
                color: Some(theme.cosmic().background(theme.transparent).on.into()),
            }
        })));
    }

    button::custom(
        layer_container(icon)
            .align_x(alignment)
            .center_y(Length::Fill),
    )
    .width(Length::Fixed(
        visualization_size.width + 2f32 * f32::from(applet_padding),
    ))
    .height(Length::Fixed(
        visualization_size.height + 2f32 * f32::from(applet_padding),
    ))
    .class(Button::AppletIcon)
    .on_press_down(Message::TogglePopup)
}
