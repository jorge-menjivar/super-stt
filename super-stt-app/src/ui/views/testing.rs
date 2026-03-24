// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::iced_widget::row;
use cosmic::widget::{self, button, settings, text};

use super::common::page_layout;
use crate::state::RecordingStatus;
use crate::ui::messages::Message;

/// Main dashboard view using cosmic-settings style
pub fn page<'a>(
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

    // Audio level display widget
    let record_button = match recording_status {
        RecordingStatus::Recording => button::standard("Recording"), // Disabled button when recording
        RecordingStatus::Idle => {
            button::standard("Test Recording").on_press(Message::StartRecording)
        }
    };

    let audio_widget = row![
        record_button,
        widget::progress_bar(
            0.0..=1.0,
            // Audio level can be a minimum of 0.1 when recording because lower than that and it can overflow when theme is fully rounded.
            audio_level.max(if audio_level > 0.0 { 0.1 } else { 0.0 })
        ),
    ]
    .align_y(Alignment::Center)
    .spacing(10);

    // Transcription output widget
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

    let sections = settings::view_column(vec![
        // Recording Test Section
        settings::section()
            .title("Recording Test")
            .add(settings::item("Status", text::body(recording_text)))
            .add(settings::flex_item("Audio Level", audio_widget))
            .add(settings::flex_item("", transcription_widget))
            .into(),
    ]);

    page_layout("Testing", sections)
}
