// SPDX-License-Identifier: GPL-3.0-only
use super::common::page_layout;
use crate::state::DaemonStatus;
use crate::ui::messages::{DaemonMessage, Message};
use cosmic::{
    Element,
    widget::{button, settings, text},
};

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
