// SPDX-License-Identifier: GPL-3.0-only
//! The Speech tab: the test utterance, and what the daemon says it is speaking.

use crate::core::app::AppModel;
use crate::daemon::client::v1::speak::{speak_command, stop_speaking_command};
use crate::state::{ErrorScope, SpeakingStatus};
use crate::ui::messages::{Message, SpeechMessage};
use cosmic::prelude::*;

impl AppModel {
    /// Route the Speech tab's messages.
    ///
    /// The optimistic badge flip that a recording UI would do is deliberately
    /// absent. The speaking status is driven only by the daemon's
    /// `speaking_state` events, because the daemon speaks for whoever asked —
    /// another client's utterance turns the badge on too, and a local flip
    /// would claim an utterance this tab did not start.
    pub(in crate::core::app) fn handle_speech_messages(
        &mut self,
        message: SpeechMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            SpeechMessage::TestTextChanged(text) => {
                self.speech.test_text = text;
                Task::none()
            }

            SpeechMessage::Speak => {
                let text = self.speech.test_text.trim().to_string();
                if text.is_empty() {
                    return Task::none();
                }
                self.clear_action_error(ErrorScope::Speech);
                Task::perform(speak_command(text), |result| match result {
                    Ok(Some(id)) => {
                        cosmic::Action::App(Message::Speech(SpeechMessage::SpeakStarted(id)))
                    }
                    // Accepted, but the daemon named no utterance. Nothing to
                    // correlate later, and nothing went wrong — stay quiet and
                    // let the events drive the badge.
                    Ok(None) => cosmic::Action::None,
                    Err(e) => cosmic::Action::App(Message::Speech(SpeechMessage::SpeakFailed(
                        e.to_string(),
                    ))),
                })
            }

            SpeechMessage::SpeakStarted(id) => {
                log::info!("speaking utterance {id}");
                Task::none()
            }

            SpeechMessage::SpeakFailed(message) => {
                self.set_action_error(ErrorScope::Speech, message);
                Task::none()
            }

            SpeechMessage::StopSpeaking => Task::perform(stop_speaking_command(), |result| {
                if let Err(e) = result {
                    log::warn!("Stop speaking failed: {e}");
                }
                cosmic::Action::None
            }),

            SpeechMessage::WidgetAudioLevel { level, is_speech } => {
                self.last_udp_data = std::time::Instant::now();
                self.speech.level = level;
                self.speech.is_playing = is_speech;
                Task::none()
            }

            SpeechMessage::WidgetSpeakingState {
                is_speaking,
                utterance_id,
            } => {
                self.last_udp_data = std::time::Instant::now();
                self.speech.status = if is_speaking {
                    SpeakingStatus::Speaking
                } else {
                    SpeakingStatus::Idle
                };
                self.speech.utterance = utterance_id;
                if is_speaking {
                    // Whatever the Voices tab was waiting for, it is done
                    // waiting: a voice is registered before its utterance is
                    // accepted, so speech starting means preparation finished.
                    //
                    // These events describe the device, not this app's
                    // requests, so another client's utterance clears the label
                    // too. That is the right trade: the alternative is matching
                    // on an utterance id the preview call does not keep, to fix
                    // a case that costs a label cleared early.
                    self.voices.previewing = None;
                    self.voices.preparing = None;
                } else {
                    // No more samples are coming, and the meter's last value
                    // would otherwise sit at whatever the final frame was.
                    self.speech.level = 0.0;
                    self.speech.is_playing = false;
                }
                Task::none()
            }
        }
    }
}
