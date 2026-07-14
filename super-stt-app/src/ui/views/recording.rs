// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::iced_widget::row;
use cosmic::widget::{self, button, settings, text};
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;

use super::common::{error_banner, page_layout};
use crate::state::RecordingStatus;
use crate::ui::messages::{
    Message, PreviewTypingMessage, RecordingMessage, RecordingStopModeMessage,
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

/// Test recording section: record button, audio level, transcription output
fn test_section<'a>(
    recording_status: &'a RecordingStatus,
    transcription_text: &'a str,
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
        widget::progress_bar(
            0.0..=1.0,
            audio_level.max(if audio_level > 0.0 { 0.1 } else { 0.0 })
        ),
    ]
    .align_y(Alignment::Center)
    .spacing(10);

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
        .add(settings::flex_item("", transcription_widget))
        .into()
}

/// Recording page: settings + test recording
pub fn page<'a>(
    recording_stop_mode: RecordingStopMode,
    preview_typing_enabled: bool,
    recording_status: &'a RecordingStatus,
    transcription_text: &'a str,
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
    blocks.push(test_section(
        recording_status,
        transcription_text,
        audio_level,
        is_speech_detected,
    ));

    page_layout("Recording", settings::view_column(blocks))
}
