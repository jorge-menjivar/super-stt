// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::{
    RecordEvent, record_command_stream, set_and_test_audio_theme, set_audio_theme, set_volume,
    stop_record_command,
};
use crate::state::{AudioTheme, ErrorScope, RecordingStatus};
use crate::ui::messages::{Message, RecordingMessage};
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
        message: RecordingMessage,
    ) -> Task<cosmic::Action<Message>> {
        match &message {
            RecordingMessage::StartRecording
            | RecordingMessage::StopRecording
            | RecordingMessage::PreviewTextReceived(_)
            | RecordingMessage::TranscriptionReceived(_) => self.handle_recording_control(message),

            RecordingMessage::AudioFeedbackToggled(_)
            | RecordingMessage::AudioThemeSelected(_)
            | RecordingMessage::AudioThemesLoaded(_)
            | RecordingMessage::VolumeChanged(_)
            | RecordingMessage::VolumeCommit
            | RecordingMessage::WidgetAudioLevel { .. }
            | RecordingMessage::WidgetRecordingState(_) => self.handle_audio_messages(message),
        }
    }

    /// Handle recording control messages: start/stop recording and transcription results.
    fn handle_recording_control(
        &mut self,
        message: RecordingMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            RecordingMessage::StartRecording => {
                if matches!(self.recording_status, RecordingStatus::Recording) {
                    return Task::none();
                }

                self.recording_status = RecordingStatus::Recording;
                self.transcription_text.clear();

                let stream = record_command_stream();
                cosmic::task::stream(stream.map(|event| match event {
                    RecordEvent::Preview(text) => cosmic::Action::App(Message::Recording(
                        RecordingMessage::PreviewTextReceived(text),
                    )),
                    RecordEvent::Final(Ok(text)) => cosmic::Action::App(Message::Recording(
                        RecordingMessage::TranscriptionReceived(text),
                    )),
                    RecordEvent::Final(Err(e)) => cosmic::Action::App(Message::Recording(
                        RecordingMessage::TranscriptionReceived(format!("Error: {e}")),
                    )),
                }))
            }

            RecordingMessage::StopRecording => Task::perform(stop_record_command(), |result| {
                if let Err(e) = result {
                    log::warn!("Stop recording failed: {e}");
                }
                cosmic::Action::None
            }),

            RecordingMessage::PreviewTextReceived(text) => {
                self.transcription_text = text;
                Task::none()
            }

            RecordingMessage::TranscriptionReceived(text) => {
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
    fn handle_audio_messages(
        &mut self,
        message: RecordingMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            RecordingMessage::AudioFeedbackToggled(enabled) => {
                // Clear any stale Customization banner as the user retries.
                self.clear_action_error(ErrorScope::Customization);
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

            RecordingMessage::AudioThemeSelected(theme) => {
                self.clear_action_error(ErrorScope::Customization);
                self.selected_audio_theme = theme;
                if theme != AudioTheme::Silent {
                    self.last_non_silent_theme = theme;
                }
                Task::perform(set_and_test_audio_theme(theme), |result| match result {
                    Ok(_) => cosmic::Action::None,
                    Err(e) => cosmic::Action::App(customization_error(&e)),
                })
            }

            RecordingMessage::AudioThemesLoaded(themes) => {
                self.audio_themes = themes;
                Task::none()
            }

            RecordingMessage::VolumeChanged(vol) => {
                // Drag tick: update the slider locally only. The daemon POST is
                // deferred to VolumeCommit (on release) so a drag doesn't fire
                // one set_volume per tick (Tier 1 #19).
                self.volume = vol;
                Task::none()
            }

            RecordingMessage::VolumeCommit => {
                self.clear_action_error(ErrorScope::Customization);
                Task::perform(set_volume(self.volume), |result| match result {
                    Ok(()) => cosmic::Action::None,
                    Err(e) => cosmic::Action::App(customization_error(&e)),
                })
            }

            RecordingMessage::WidgetAudioLevel { level, is_speech } => {
                self.last_udp_data = std::time::Instant::now();
                self.audio_level = level;
                self.is_speech_detected = is_speech;
                Task::none()
            }

            RecordingMessage::WidgetRecordingState(is_recording) => {
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
