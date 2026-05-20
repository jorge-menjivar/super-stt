// SPDX-License-Identifier: GPL-3.0-only
use crate::{app::Message, models::state::DaemonConnectionState};
use cosmic::{
    Element,
    iced::widget::column,
    widget::{button, text},
};

pub fn create_status_section(daemon_state: &DaemonConnectionState) -> Element<'static, Message> {
    match daemon_state {
        DaemonConnectionState::Error(e) => column![
            text(e.clone()).size(12),
            text("The daemon may still be starting").size(10)
        ]
        .spacing(4)
        .into(),
        DaemonConnectionState::Connected => column![text("Connected").size(12)].spacing(4).into(),
        DaemonConnectionState::Connecting => column![
            text("Connecting to daemon...").size(12),
            text("The daemon may still be starting").size(10)
        ]
        .spacing(4)
        .into(),
        DaemonConnectionState::Blocked(reason) => column![
            text("Authorization denied").size(12),
            text(format!("Reason: {reason}")).size(10),
            text("Restart the daemon to clear the deny cache:").size(10),
            text("  systemctl --user restart super-stt").size(10),
            text("then click below to request authorization again.").size(10),
            button::standard("Retry authorization").on_press(Message::RetryAuthorization),
        ]
        .spacing(6)
        .into(),
    }
}
