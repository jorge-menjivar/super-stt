// SPDX-License-Identifier: GPL-3.0-only
//! The Read aloud tab: where the read-aloud shortcut reads from, and a test
//! that reads it now. Text to speech's counterpart to Input Simulation, which
//! holds the write method and its typing test.

use cosmic::Element;
use cosmic::widget::{self, button, settings, text};
use super_voice_shared::models::read_method::ReadMethod;

use super::common::error_banner;
use crate::state::SpeakingStatus;
use crate::ui::messages::{Message, ReadMethodMessage, SpeechMessage};

/// Where the shortcut reads from.
fn settings_section(read_method: ReadMethod) -> Element<'static, Message> {
    let methods = ReadMethod::ALL;
    let names: Vec<String> = methods
        .iter()
        .map(|m| crate::i18n::read_method_name(*m))
        .collect();
    let selected = methods.iter().position(|m| *m == read_method);
    settings::section()
        .title(crate::fl!("settings"))
        .add(
            settings::item::builder(crate::fl!("read-from"))
                .description(crate::fl!("read-from-description"))
                .control(widget::dropdown(names, selected, move |index| {
                    Message::ReadMethod(ReadMethodMessage::Changed(methods[index]))
                })),
        )
        .into()
}

/// Read what the setting names now, the way the shortcut does, or stop.
fn test_section(
    read_method: ReadMethod,
    speaking_status: &SpeakingStatus,
) -> Element<'static, Message> {
    let status = match speaking_status {
        SpeakingStatus::Speaking => crate::fl!("speaking-status"),
        SpeakingStatus::Idle => crate::fl!("not-speaking-status"),
    };
    // Stop while anything is speaking, like the shortcut's second press.
    let action = match speaking_status {
        SpeakingStatus::Speaking => button::destructive(crate::fl!("stop"))
            .on_press(Message::Speech(SpeechMessage::StopSpeaking)),
        SpeakingStatus::Idle => button::suggested(crate::fl!("read"))
            .on_press(Message::ReadMethod(ReadMethodMessage::Test)),
    };
    let hint = match read_method {
        ReadMethod::Selection => crate::fl!("read-test-selection"),
        ReadMethod::Clipboard => crate::fl!("read-test-clipboard"),
    };
    settings::section()
        .title(crate::fl!("test"))
        .add(settings::item(crate::fl!("status"), text::body(status)))
        .add(
            settings::item::builder(crate::fl!("read-test"))
                .description(hint)
                .control(action),
        )
        .into()
}

pub fn page<'a>(
    read_method: ReadMethod,
    speaking_status: &SpeakingStatus,
    action_error: Option<&'a str>,
) -> Element<'a, Message> {
    let mut blocks = Vec::new();
    if let Some(message) = action_error {
        blocks.push(error_banner(message));
    }
    blocks.push(settings_section(read_method));
    blocks.push(test_section(read_method, speaking_status));
    settings::view_column(blocks).into()
}
