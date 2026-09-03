// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::widget::row;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, settings, text};
use super_stt_shared::models::notification_method::NotificationMethod;
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;

use super::common::{error_banner, page_layout};
use crate::state::RecordingStatus;
use crate::ui::messages::{
    Message, NotificationMethodMessage, PreviewTypingMessage, RecordingMessage,
    RecordingStopModeMessage,
};

/// Recording settings section: stop mode + preview typing
fn settings_section(
    recording_stop_mode: RecordingStopMode,
    preview_typing_enabled: bool,
) -> Element<'static, Message> {
    let modes = [
        RecordingStopMode::SilenceOnly,
        RecordingStopMode::SilenceAndManual,
        RecordingStopMode::ManualOnly,
    ];
    let mode_names: Vec<String> = modes.iter().map(|m| m.pretty_name().to_string()).collect();
    let selected_index = modes.iter().position(|m| *m == recording_stop_mode);

    settings::section()
        .title("Settings")
        .add(
            settings::item::builder("Stop Mode")
                .description("Controls how to stop transcribing")
                .control(widget::dropdown(mode_names, selected_index, move |index| {
                    Message::RecordingStopMode(RecordingStopModeMessage::Changed(modes[index]))
                })),
        )
        .add(
            settings::item::builder("Preview Typing")
                .description(
                    "Shows transcription as you speak. Experimental, may affect performance.",
                )
                .control(
                    cosmic::widget::toggler(preview_typing_enabled)
                        .on_toggle(|b| Message::PreviewTyping(PreviewTypingMessage::Toggled(b))),
                ),
        )
        .into()
}

/// Notifications section: how recording failures reach the user
fn notifications_section(notification_method: NotificationMethod) -> Element<'static, Message> {
    let methods = [
        NotificationMethod::Auto,
        NotificationMethod::Dbus,
        NotificationMethod::Typed,
        NotificationMethod::Off,
    ];
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
                    "How recording failures are reported. Desktop notifications \
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

/// How much of the live preview to keep on screen. Sized to fill the row
/// without wrapping at the default window width.
const LIVE_LINE_CHARS: usize = 90;

/// The last `max_chars` characters of `text`, prefixed with an ellipsis when
/// something was dropped. Counts chars, not bytes — a multibyte transcript must
/// not be sliced mid-character.
fn live_tail(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let tail: String = text.chars().skip(total - max_chars).collect();
    format!("\u{2026}{tail}")
}

/// Test recording section: record button, audio level, live preview line,
/// transcription output
fn test_section<'a>(
    recording_status: &'a RecordingStatus,
    transcription_text: &'a str,
    preview_text: &'a str,
    audio_level: f32,
    is_speech_detected: bool,
) -> Element<'a, Message> {
    let recording_text = match recording_status {
        RecordingStatus::Recording => {
            if is_speech_detected {
                "🎤 Speech"
            } else {
                "🔇 Silence"
            }
        }
        RecordingStatus::Idle => "⏹️ Not recording",
    };

    let record_button = match recording_status {
        RecordingStatus::Recording => button::destructive("Stop Recording")
            .on_press(Message::Recording(RecordingMessage::StopRecording)),
        RecordingStatus::Idle => button::suggested("Test Recording")
            .on_press(Message::Recording(RecordingMessage::StartRecording)),
    };

    let audio_widget = row![
        record_button,
        widget::determinate_linear(audio_level.max(if audio_level > 0.0 { 0.1 } else { 0.0 }))
            .width(Length::Fill),
    ]
    .align_y(Alignment::Center)
    .spacing(10);

    // One line that keeps changing: the newest incremental text, held to a
    // single row so the section doesn't reflow on every update. Previews grow
    // word by word, so the tail is the part worth showing.
    let live_widget = {
        let content = if preview_text.is_empty() {
            "\u{2014}".to_string()
        } else {
            live_tail(preview_text, LIVE_LINE_CHARS)
        };
        widget::container(
            text::body(content)
                .wrapping(cosmic::iced::widget::text::Wrapping::None)
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .clip(true)
    };

    let transcription_widget = {
        let content = if transcription_text.is_empty() {
            "Transcriptions will appear here after test recordings...".to_string()
        } else {
            transcription_text.to_string()
        };

        widget::scrollable(
            widget::container(text::body(content))
                .padding(15)
                .width(Length::Fill),
        )
        .height(Length::Fixed(60.0))
        .width(Length::Fill)
    };

    settings::section()
        .title("Test")
        .add(settings::item("Status", text::body(recording_text)))
        .add(settings::flex_item("Audio Level", audio_widget))
        .add(settings::flex_item("Live", live_widget))
        .add(settings::flex_item("", transcription_widget))
        .into()
}

/// Recording page: settings + test recording
// reason: one parameter per piece of view state the page renders; grouping them would only add indirection.
#[allow(clippy::too_many_arguments)]
pub fn page<'a>(
    recording_stop_mode: RecordingStopMode,
    preview_typing_enabled: bool,
    notification_method: NotificationMethod,
    recording_status: &'a RecordingStatus,
    transcription_text: &'a str,
    preview_text: &'a str,
    audio_level: f32,
    is_speech_detected: bool,
    action_error: Option<&'a str>,
) -> Element<'a, Message> {
    let mut blocks = Vec::new();
    if let Some(message) = action_error {
        blocks.push(error_banner(message));
    }
    blocks.push(settings_section(
        recording_stop_mode,
        preview_typing_enabled,
    ));
    blocks.push(notifications_section(notification_method));
    blocks.push(test_section(
        recording_status,
        transcription_text,
        preview_text,
        audio_level,
        is_speech_detected,
    ));

    page_layout("Recording", settings::view_column(blocks))
}
