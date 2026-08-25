// SPDX-License-Identifier: GPL-3.0-only
use std::rc::Rc;

use cosmic::{
    Element,
    iced::{Alignment, Length, widget as iced_widget},
    theme::{self, Button},
    widget::{self, button, container, layer_container, mouse_area},
};

use super::SuperSttApplet;
use super::layout::AppletLayout;
use crate::app::Message;
use crate::models::state::{DaemonConnectionState, RecordingState};
use crate::ui::views::{PopupContentParams, create_popup_content};

// Cache icon bytes to avoid allocation on every render.
static NORMAL_ICON: &[u8] = include_bytes!("../../resources/assets/super-stt-icon.svg");
static TRANSPARENT_ICON: &[u8] = include_bytes!("../../resources/assets/transparent-icon.svg");
static ERROR_ICON: &[u8] = include_bytes!("../../resources/assets/error-icon.svg");

impl SuperSttApplet {
    pub(super) fn view_applet(&self) -> Element<'_, Message> {
        // Show visualizations only when the daemon is actively recording
        // and the user has visualizations enabled.
        let should_show_visualizations = matches!(self.recording_state, RecordingState::Recording)
            && self.config.ui.show_visualization;
        let should_show_working = matches!(self.recording_state, RecordingState::Processing)
            && self.config.ui.show_visualization;

        // One box for every state, so switching between the icon and the
        // visualizations never resizes the panel around us.
        let layout = AppletLayout::compute(
            &self.core.applet,
            self.config.ui.show_visualization,
            self.config.ui.applet_width,
        );

        if self.daemon_state == DaemonConnectionState::Connected && should_show_visualizations {
            // The press handler wraps the padded box, not the canvas inside it,
            // so the whole surface toggles the popup — the icon button does the
            // same across its own padding.
            let visualization_element = mouse_area(
                container(self.visualization.clone())
                    .width(Length::Fixed(layout.total.width))
                    .height(Length::Fixed(layout.total.height))
                    .padding(layout.padding),
            )
            .on_press(Message::TogglePopup);

            self.core
                .applet
                .autosize_window(visualization_element)
                .into()
        } else if self.daemon_state == DaemonConnectionState::Connected && should_show_working {
            let working_element = mouse_area(
                container(self.working_animation.clone())
                    .width(Length::Fixed(layout.total.width))
                    .height(Length::Fixed(layout.total.height))
                    .padding(layout.padding),
            )
            .on_press(Message::TogglePopup);

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

            let icon_alignment = self.config.ui.icon_alignment.to_alignment();

            let icon_button =
                transparent_icon_button(icon_bytes, symbolic, &layout, icon_alignment);

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
}

/// Build the panel button wrapping `icon_bytes`.
///
/// `alignment` runs along the panel — horizontally on a top or bottom panel,
/// vertically on a side one — so the glyph can be lined up with a neighbouring
/// side applet's visualization. The cross axis is always centered.
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
    layout: &AppletLayout,
    alignment: Alignment,
) -> cosmic::widget::Button<'a, Message> {
    let icon_size = (layout.content.height.min(layout.content.width) * 0.6).clamp(16.0, 32.0);

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

    let aligned = if layout.horizontal {
        layer_container(icon)
            .align_x(alignment)
            .center_y(Length::Fill)
    } else {
        layer_container(icon)
            .align_y(alignment)
            .center_x(Length::Fill)
    };

    button::custom(aligned)
        .width(Length::Fixed(layout.total.width))
        .height(Length::Fixed(layout.total.height))
        // Same inset as the visualizations get, so an aligned icon sits exactly
        // where the visualization's edge would be.
        .padding(layout.padding)
        .class(Button::AppletIcon)
        .on_press_down(Message::TogglePopup)
}
