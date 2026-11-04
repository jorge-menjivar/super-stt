// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::widget::row;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, settings, text};
use super_tts_shared::models::notification_method::NotificationMethod;

use super::common::{error_banner, page_layout};
use crate::state::SpeakingStatus;
use crate::ui::messages::{Message, NotificationMethodMessage, SpeechMessage};

/// Notifications section: how synthesis failures reach the user.
fn notifications_section(notification_method: NotificationMethod) -> Element<'static, Message> {
    let methods = [NotificationMethod::Auto, NotificationMethod::Off];
    let method_names: Vec<String> = methods
        .iter()
        .map(|m| m.pretty_name().to_string())
        .collect();
    let selected_index = methods.iter().position(|m| *m == notification_method);

    settings::section()
        .title("Notifications")
        .add(
            settings::item::builder("Failure Notices")
                .description(
                    "How synthesis failures are reported. Desktop notifications \
                     work on any desktop with a notification server.",
                )
                .control(widget::dropdown(
                    method_names,
                    selected_index,
                    move |index| {
                        Message::NotificationMethod(NotificationMethodMessage::Changed(
                            methods[index],
                        ))
                    },
                )),
        )
        .into()
}

/// Test speech section: a text field, speak/stop, and the live output level.
fn test_section<'a>(
    speaking_status: &'a SpeakingStatus,
    test_text: &'a str,
    audio_level: f32,
    is_playing: bool,
) -> Element<'a, Message> {
    let status_text = match speaking_status {
        SpeakingStatus::Speaking => {
            if is_playing {
                "🔊 Speaking"
            } else {
                // Queued but nothing has reached the device yet: the daemon
                // answers as soon as an utterance is accepted, which is before
                // the first sample is rendered.
                "⏳ Starting"
            }
        }
        SpeakingStatus::Idle => "⏹️ Not speaking",
    };

    // Speak is disabled on empty text rather than sending it: the daemon
    // rejects an empty utterance with a coded 400, and a button that only ever
    // produces an error banner is worse than one that is visibly unavailable.
    let action_button = match speaking_status {
        SpeakingStatus::Speaking => {
            button::destructive("Stop").on_press(Message::Speech(SpeechMessage::StopSpeaking))
        }
        SpeakingStatus::Idle => {
            let b = button::suggested("Speak");
            if test_text.trim().is_empty() {
                b
            } else {
                b.on_press(Message::Speech(SpeechMessage::Speak))
            }
        }
    };

    let input = widget::text_input("Type something to hear it…", test_text)
        .on_input(|s| Message::Speech(SpeechMessage::TestTextChanged(s)))
        .width(Length::Fill);

    let level_widget = row![
        action_button,
        widget::determinate_linear(audio_level.max(if audio_level > 0.0 { 0.1 } else { 0.0 }))
            .width(Length::Fill),
    ]
    .align_y(Alignment::Center)
    .spacing(10);

    settings::section()
        .title("Test")
        .add(settings::item("Status", text::body(status_text)))
        .add(settings::flex_item("Text", input))
        .add(settings::flex_item("Output Level", level_widget))
        .into()
}

/// Speech page: notification settings + a test utterance.
pub fn page<'a>(
    notification_method: NotificationMethod,
    speaking_status: &'a SpeakingStatus,
    test_text: &'a str,
    audio_level: f32,
    is_playing: bool,
    action_error: Option<&'a str>,
) -> Element<'a, Message> {
    let mut blocks = Vec::new();
    if let Some(message) = action_error {
        blocks.push(error_banner(message));
    }
    blocks.push(notifications_section(notification_method));
    blocks.push(test_section(
        speaking_status,
        test_text,
        audio_level,
        is_playing,
    ));

    page_layout("Speech", settings::view_column(blocks))
}
