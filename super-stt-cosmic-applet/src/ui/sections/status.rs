// SPDX-License-Identifier: GPL-3.0-only
use crate::{app::Message, models::state::DaemonConnectionState};
use cosmic::{Element, iced::widget::column, widget::text};

pub fn create_status_section(daemon_state: &DaemonConnectionState) -> Element<'static, Message> {
    // Create status section with optional retry button
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
    }
}
