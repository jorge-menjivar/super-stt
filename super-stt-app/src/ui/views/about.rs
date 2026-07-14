// SPDX-License-Identifier: GPL-3.0-only
//! About page view for the Super STT application.

use cosmic::iced::Alignment;
use cosmic::iced_widget::column;
use cosmic::widget::{self, button};
use cosmic::{Element, cosmic_theme, theme};

use crate::ui::messages::{Message, ShellMessage};

const APP_ICON: &[u8] =
    include_bytes!("../../../resources/icons/hicolor/scalable/apps/super-stt-app.svg");
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// About page view
pub fn page() -> Element<'static, Message> {
    let cosmic_theme::Spacing { space_xxs, .. } = theme::active().cosmic().spacing;

    let icon = widget::svg(widget::svg::Handle::from_memory(APP_ICON));

    let title = widget::text::title3("Super STT");

    let hash = env!("VERGEN_GIT_SHA");
    let short_hash: String = hash.chars().take(7).collect();
    let date = env!("VERGEN_GIT_COMMIT_DATE");

    let link = widget::button::link(REPOSITORY)
        .on_press(Message::Shell(ShellMessage::OpenRepositoryUrl))
        .padding(0);

    column![
        icon,
        title,
        link,
        button::link(format!("Git: {short_hash} ({date})"))
            .on_press(Message::Shell(ShellMessage::LaunchUrl(format!(
                "{REPOSITORY}/commits/{hash}"
            ))))
            .padding(0),
    ]
    .align_x(Alignment::Center)
    .spacing(space_xxs)
    .into()
}
