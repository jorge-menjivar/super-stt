// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::{
    set_and_test_audio_theme, set_audio_theme, set_volume, speak_command, stop_speaking_command,
};
use crate::state::{AudioTheme, ErrorScope, SpeakingStatus};
use crate::ui::messages::{Message, SpeechMessage};
use cosmic::prelude::*;
use super_tts_shared::daemon::http_client::HttpError;

/// Build the audio-theme rollback message: restore the captured pre-save theme
/// fields and raise a Customization banner (audit Tier 3 #37).
fn audio_theme_save_failed(
    prev_selected: AudioTheme,
    prev_non_silent: AudioTheme,
    e: &HttpError,
) -> Message {
    Message::Speech(SpeechMessage::AudioThemeSaveFailed {
        prev_selected,
        prev_non_silent,
        message: e.to_string(),
    })
}

impl AppModel {
    /// Route speech/audio messages to the appropriate helper.
    pub(in crate::core::app) fn handle_speech_messages(
        &mut self,
        message: SpeechMessage,
    ) -> Task<cosmic::Action<Message>> {
        match &message {
            SpeechMessage::TestTextChanged(_)
            | SpeechMessage::Speak
            | SpeechMessage::SpeakStarted(_)
            | SpeechMessage::SpeakFailed(_)
            | SpeechMessage::StopSpeaking => self.handle_speech_control(message),

            SpeechMessage::AudioFeedbackToggled(_)
            | SpeechMessage::AudioThemeSelected(_)
            | SpeechMessage::AudioThemeSaveFailed { .. }
            | SpeechMessage::AudioThemesLoaded(_)
            | SpeechMessage::VolumeChanged(_)
            | SpeechMessage::VolumeCommit
            | SpeechMessage::VolumeSaveFailed { .. } => self.handle_audio_messages(message),

            SpeechMessage::WidgetAudioLevel { .. } | SpeechMessage::WidgetSpeakingState { .. } => {
                self.handle_widget_messages(message)
            }
        }
    }

    /// Handle speech control messages: the test field, and starting/stopping an
    /// utterance.
    ///
    /// The optimistic badge flip that a recording UI would do is deliberately
    /// absent. `speaking_status` is driven only by the daemon's
    /// `speaking_state` events, because the daemon speaks for whoever asked —
    /// another client's utterance turns the badge on too, and a local flip
    /// would claim an utterance this page did not start.
    fn handle_speech_control(&mut self, message: SpeechMessage) -> Task<cosmic::Action<Message>> {
        match message {
            SpeechMessage::TestTextChanged(text) => {
                self.speech_test_text = text;
                Task::none()
            }

            SpeechMessage::Speak => {
                let text = self.speech_test_text.trim().to_string();
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

            _ => Task::none(),
        }
    }

    /// Handle audio-theme and volume settings messages.
    fn handle_audio_messages(&mut self, message: SpeechMessage) -> Task<cosmic::Action<Message>> {
        match message {
            SpeechMessage::AudioFeedbackToggled(enabled) => {
                // Clear any stale Customization banner as the user retries.
                self.clear_action_error(ErrorScope::Customization);
                // Capture the pre-optimistic values so a failed save can roll
                // the UI back instead of leaving it stuck on the value the
                // daemon rejected (audit Tier 3 #37).
                let prev_selected = self.selected_audio_theme;
                let prev_non_silent = self.last_non_silent_theme;
                let theme = if enabled {
                    self.last_non_silent_theme
                } else {
                    AudioTheme::Silent
                };
                self.selected_audio_theme = theme;
                // Success is a no-op (the optimistic value already holds); a
                // failure restores the prior value and surfaces a scoped banner
                // instead of flipping the whole app to the connection-error page
                // (Tier 1 #13). Reusing `DaemonConnected` here also used to
                // trigger a full settings refetch on every toggle (Tier 1 #14).
                Task::perform(set_audio_theme(theme), move |result| match result {
                    Ok(_) => cosmic::Action::None,
                    Err(e) => cosmic::Action::App(audio_theme_save_failed(
                        prev_selected,
                        prev_non_silent,
                        &e,
                    )),
                })
            }

            SpeechMessage::AudioThemeSelected(theme) => {
                self.clear_action_error(ErrorScope::Customization);
                let prev_selected = self.selected_audio_theme;
                let prev_non_silent = self.last_non_silent_theme;
                self.selected_audio_theme = theme;
                if theme != AudioTheme::Silent {
                    self.last_non_silent_theme = theme;
                }
                Task::perform(
                    set_and_test_audio_theme(theme),
                    move |result| match result {
                        Ok(_) => cosmic::Action::None,
                        Err(e) => cosmic::Action::App(audio_theme_save_failed(
                            prev_selected,
                            prev_non_silent,
                            &e,
                        )),
                    },
                )
            }

            SpeechMessage::AudioThemeSaveFailed {
                prev_selected,
                prev_non_silent,
                message,
            } => {
                self.selected_audio_theme = prev_selected;
                self.last_non_silent_theme = prev_non_silent;
                self.set_action_error(ErrorScope::Customization, message);
                Task::none()
            }

            SpeechMessage::AudioThemesLoaded(themes) => {
                self.audio_themes = themes;
                Task::none()
            }

            SpeechMessage::VolumeChanged(vol) => {
                // Drag tick: update the slider locally only. The daemon POST is
                // deferred to VolumeCommit (on release) so a drag doesn't fire
                // one set_volume per tick (Tier 1 #19).
                self.volume = vol;
                Task::none()
            }

            SpeechMessage::VolumeCommit => {
                self.clear_action_error(ErrorScope::Customization);
                // The drag already clobbered `self.volume`, so the rollback
                // target is the last committed value. Advance it optimistically
                // and capture the prior committed value for a possible rollback
                // (audit Tier 3 #37).
                let prev_volume = self.last_committed_volume;
                let new_volume = self.volume;
                self.last_committed_volume = new_volume;
                Task::perform(set_volume(new_volume), move |result| match result {
                    Ok(()) => cosmic::Action::None,
                    Err(e) => {
                        cosmic::Action::App(Message::Speech(SpeechMessage::VolumeSaveFailed {
                            prev_volume,
                            message: e.to_string(),
                        }))
                    }
                })
            }

            SpeechMessage::VolumeSaveFailed {
                prev_volume,
                message,
            } => {
                self.volume = prev_volume;
                self.last_committed_volume = prev_volume;
                self.set_action_error(ErrorScope::Customization, message);
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle the SSE-driven messages: what the daemon says it is doing, as
    /// opposed to what this page asked it to do.
    ///
    /// Separate from the settings handler above because the direction of travel
    /// is opposite — nothing here initiates anything, it only reflects the
    /// device's state, including utterances this app never started.
    fn handle_widget_messages(&mut self, message: SpeechMessage) -> Task<cosmic::Action<Message>> {
        match message {
            SpeechMessage::WidgetAudioLevel { level, is_speech } => {
                self.last_udp_data = std::time::Instant::now();
                self.audio_level = level;
                self.is_speech_detected = is_speech;
                Task::none()
            }

            SpeechMessage::WidgetSpeakingState {
                is_speaking,
                utterance_id,
            } => {
                self.last_udp_data = std::time::Instant::now();
                self.speaking_status = if is_speaking {
                    SpeakingStatus::Speaking
                } else {
                    SpeakingStatus::Idle
                };
                self.speaking_utterance = utterance_id;
                if is_speaking {
                    // Whatever the Voices page was waiting for, it is done
                    // waiting: a voice is registered before its utterance is
                    // accepted, so speech starting means preparation finished.
                    //
                    // This rather than the request returning, which is what the
                    // page used to wait for: `POST /speak` answers once the
                    // *last* sample is queued, so holding "Preparing…" until
                    // then left the label up for seconds of audible speech.
                    //
                    // These events describe the device, not this app's
                    // requests, so another client's utterance clears the label
                    // too. That is the right trade: the alternative is matching
                    // on an utterance id the preview call does not keep, to fix
                    // a case that costs a label cleared early.
                    self.voices.previewing = None;
                    self.voices.preparing = None;
                }
                if !is_speaking {
                    // No more samples are coming, and the meter's last value
                    // would otherwise sit at whatever the final frame was.
                    self.audio_level = 0.0;
                    self.is_speech_detected = false;
                }
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
