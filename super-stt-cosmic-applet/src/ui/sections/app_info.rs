// SPDX-License-Identifier: GPL-3.0-only
use crate::app::Message;
use cosmic::{
    Element,
    iced::{Alignment, Length, widget::row},
    theme,
    widget::{Space, button, icon, text},
};

// Cache GitHub icon bytes to avoid allocation on every render
static GITHUB_ICON_DARK: &[u8] =
    include_bytes!("../../../resources/assets/github-mark/github-mark-white.svg");
static GITHUB_ICON_LIGHT: &[u8] =
    include_bytes!("../../../resources/assets/github-mark/github-mark.svg");

pub fn create_app_info_section() -> Element<'static, Message> {
    let current_theme = theme::active();
    let spacing = current_theme.cosmic().spacing;

    // Check if dark theme is active
    let is_dark = current_theme.cosmic().is_dark;

    // Choose appropriate GitHub icon based on theme
    let github_icon = if is_dark {
        GITHUB_ICON_DARK
    } else {
        GITHUB_ICON_LIGHT
    };

    row![
        // Left side: Super STT title with small version
        row![
            text("Super STT").size(16),
            text::caption(format!("v{}", crate::VERSION)),
        ]
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center),
        // Spacer to push GitHub button to the right
        Space::new().width(Length::Fill),
        // Right side: GitHub button
        cosmic::widget::tooltip(
            button::icon(icon::from_svg_bytes(github_icon))
                .on_press(Message::OpenGitHub)
                .padding(4),
            "View on GitHub",
            cosmic::widget::tooltip::Position::Bottom
        ),
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}
