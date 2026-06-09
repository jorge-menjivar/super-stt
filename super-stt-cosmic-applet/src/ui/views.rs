// SPDX-License-Identifier: GPL-3.0-only
use crate::{
    app::Message,
    config::AppletConfig,
    models::{
        state::{DaemonConnectionState, IsOpen},
        theme::ThemeConfig,
    },
    ui::sections::{
        app_info::create_app_info_section, launch::create_launch_section,
        settings::section::create_applet_settings_section, status::create_status_section,
    },
};
use cosmic::{
    Apply, Element,
    applet::{menu_control_padding, padded_control},
    iced::widget::column,
    theme,
    widget::{divider, segmented_button::SingleSelectModel},
};

/// Parameters for creating popup content to avoid too many function arguments
pub struct PopupContentParams<'a> {
    pub daemon_state: &'a DaemonConnectionState,
    pub is_open: &'a IsOpen,
    pub theme_config: &'a ThemeConfig,
    pub config: &'a AppletConfig,
    pub icon_alignment_model: &'a SingleSelectModel,
    pub theme_selector_model: &'a SingleSelectModel,
    pub selected_theme_for_config: bool,
}

pub fn create_popup_content<'a>(params: &PopupContentParams<'a>) -> Element<'a, Message> {
    let spacing = theme::active().cosmic().spacing;

    column![
        padded_control(create_app_info_section())
            .padding(menu_control_padding())
            .apply(Element::from),
        padded_control(divider::horizontal::default())
            .padding([spacing.space_xs, spacing.space_s])
            .apply(Element::from),
        // Only show Super STT controls when connected to the daemon
        if matches!(params.daemon_state, DaemonConnectionState::Connected) {
            create_applet_settings_section(
                params.config,
                params.theme_config,
                params.is_open,
                params.icon_alignment_model,
                params.theme_selector_model,
                params.selected_theme_for_config,
            )
        } else {
            padded_control(create_status_section(params.daemon_state))
                .padding(menu_control_padding())
                .apply(Element::from)
        },
        // Add divider before launch section
        padded_control(divider::horizontal::default())
            .padding([spacing.space_xs, spacing.space_s])
            .apply(Element::from),
        // Launch button section at the bottom
        create_launch_section()
    ]
    .padding([8, 0])
    .into()
}
