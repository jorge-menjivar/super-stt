// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::{
    RecordEvent, record_command_stream, set_and_test_audio_theme, set_audio_theme, set_volume,
    stop_record_command,
};
use crate::state::{AudioTheme, ErrorScope, RecordingStatus};
use crate::ui::messages::Message;
use cosmic::prelude::*;
use futures_util::StreamExt;
use super_stt_shared::daemon::http_client::HttpError;

/// Build a Customization-scoped banner message for a failed audio-settings save.
fn customization_error(e: &HttpError) -> Message {
    Message::SettingActionFailed {
        scope: ErrorScope::Customization,
        message: e.to_string(),
    }
}

impl AppModel {
    /// Route recording/audio messages to the appropriate helper.
    pub(in crate::core::app) fn handle_recording_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match &message {
            Message::StartRecording
            | Message::StopRecording
            | Message::PreviewTextReceived(_)
            | Message::TranscriptionReceived(_) => self.handle_recording_control(message),

            Message::AudioFeedbackToggled(_)
            | Message::AudioThemeSelected(_)
            | Message::AudioThemesLoaded(_)
            | Message::VolumeChanged(_)
            | Message::WidgetAudioLevel { .. }
            | Message::WidgetRecordingState(_) => self.handle_audio_messages(message),

            _ => Task::none(),
        }
    }

    /// Handle recording control messages: start/stop recording and transcription results.
    fn handle_recording_control(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::StartRecording => {
                if matches!(self.recording_status, RecordingStatus::Recording) {
                    return Task::none();
                }

                self.recording_status = RecordingStatus::Recording;
                self.transcription_text.clear();

                let stream = record_command_stream();
                cosmic::task::stream(stream.map(|event| match event {
                    RecordEvent::Preview(text) => {
                        cosmic::Action::App(Message::PreviewTextReceived(text))
                    }
                    RecordEvent::Final(Ok(text)) => {
                        cosmic::Action::App(Message::TranscriptionReceived(text))
                    }
                    RecordEvent::Final(Err(e)) => {
                        cosmic::Action::App(Message::TranscriptionReceived(format!("Error: {e}")))
                    }
                }))
            }

            Message::StopRecording => Task::perform(stop_record_command(), |result| {
                if let Err(e) = result {
                    log::warn!("Stop recording failed: {e}");
                }
                cosmic::Action::None
            }),

            Message::PreviewTextReceived(text) => {
                self.transcription_text = text;
                Task::none()
            }

            Message::TranscriptionReceived(text) => {
                log::info!(
                    "TranscriptionReceived: '{}'",
                    text.chars().take(50).collect::<String>()
                );
                self.transcription_text = text;
                self.recording_status = RecordingStatus::Idle;
                self.audio_level = 0.0;
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle audio-level, theme, and volume messages.
    fn handle_audio_messages(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::AudioFeedbackToggled(enabled) => {
                // Clear any stale Customization banner as the user retries.
                self.action_error = None;
                let theme = if enabled {
                    self.last_non_silent_theme
                } else {
                    AudioTheme::Silent
                };
                self.selected_audio_theme = theme;
                // Success is a no-op (the optimistic value already holds); a
                // failure surfaces a scoped banner instead of flipping the whole
                // app to the connection-error page (Tier 1 #13). Reusing
                // `DaemonConnected` here also used to trigger a full settings
                // refetch on every toggle (Tier 1 #14).
                Task::perform(set_audio_theme(theme), |result| match result {
                    Ok(_) => cosmic::Action::None,
                    Err(e) => cosmic::Action::App(customization_error(&e)),
                })
            }

            Message::AudioThemeSelected(theme) => {
                self.action_error = None;
                self.selected_audio_theme = theme;
                if theme != AudioTheme::Silent {
                    self.last_non_silent_theme = theme;
                }
                Task::perform(set_and_test_audio_theme(theme), |result| match result {
                    Ok(_) => cosmic::Action::None,
                    Err(e) => cosmic::Action::App(customization_error(&e)),
                })
            }

            Message::AudioThemesLoaded(themes) => {
                self.audio_themes = themes;
                Task::none()
            }

            Message::VolumeChanged(vol) => {
                self.action_error = None;
                self.volume = vol;
                Task::perform(set_volume(vol), |result| match result {
                    Ok(()) => cosmic::Action::None,
                    Err(e) => cosmic::Action::App(customization_error(&e)),
                })
            }

            Message::WidgetAudioLevel { level, is_speech } => {
                self.last_udp_data = std::time::Instant::now();
                self.audio_level = level;
                self.is_speech_detected = is_speech;
                Task::none()
            }

            Message::WidgetRecordingState(is_recording) => {
                self.last_udp_data = std::time::Instant::now();
                self.recording_status = if is_recording {
                    RecordingStatus::Recording
                } else {
                    RecordingStatus::Idle
                };
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
