// SPDX-License-Identifier: GPL-3.0-only
use super::common::page_layout;
use crate::state::DaemonStatus;
use crate::ui::messages::{DaemonMessage, Message, ShellMessage};
use cosmic::iced::Alignment;
use cosmic::iced::widget::row;
use cosmic::{
    Element,
    widget::{button, settings, text},
};

/// What fixes a daemon that is installed but not running — which is the only
/// way this page has anything to say, since the app and the daemon install
/// together.
const START_DAEMON: &str = "systemctl --user start super-stt";

/// Settings page view using cosmic-settings style
pub fn page(daemon_status: &DaemonStatus, socket_path: String) -> Element<'_, Message> {
    let status_text = match daemon_status {
        DaemonStatus::Connected => "✅ Connected".to_string(),
        DaemonStatus::Connecting => "⏳ Connecting...".to_string(),
        DaemonStatus::Disconnected => "❌ Disconnected".to_string(),
        DaemonStatus::Error(err) => format!("❌ Error: {err}"),
        DaemonStatus::Blocked(reason) => format!("⛔ Authorization denied ({reason})"),
    };

    let mut connection_section = settings::section()
        .title("Connection Information")
        .add(settings::item("Connection", text::body(status_text)))
        .add(settings::item("Socket Path", text::body(socket_path)));

    // Not reaching the daemon is the one failure this page can actually help
    // with, and it never said how. A red status and nothing to act on is not
    // an answer, so offer the command that fixes it, ready to paste.
    if matches!(
        daemon_status,
        DaemonStatus::Disconnected | DaemonStatus::Error(_)
    ) {
        connection_section = connection_section
            .add(settings::item(
                "No daemon",
                text::body("The daemon is not running."),
            ))
            .add(settings::item(
                "Start it",
                row![
                    text::body(START_DAEMON),
                    button::standard("Copy").on_press(Message::Shell(ShellMessage::CopyText(
                        START_DAEMON.to_string()
                    ))),
                ]
                .spacing(cosmic::theme::spacing().space_xs)
                .align_y(Alignment::Center),
            ));
    }

    if matches!(daemon_status, DaemonStatus::Blocked(_)) {
        connection_section = connection_section
            .add(settings::item(
                "Action required",
                text::body(
                    "Authorization was denied. Restart the daemon to clear the deny \
                     cache (systemctl --user restart super-stt), then click Retry to \
                     request access again.",
                ),
            ))
            .add(settings::item(
                "",
                button::standard("Retry authorization")
                    .on_press(Message::Daemon(DaemonMessage::RetryAuthorization)),
            ));
    }

    let sections = vec![connection_section.into()];

    let sections_view = settings::view_column(sections);
    page_layout("Connection", sections_view)
}
