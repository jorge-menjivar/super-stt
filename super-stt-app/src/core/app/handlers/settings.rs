// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::{set_preview_typing, set_recording_stop_mode, set_write_method};
use crate::ui::messages::{
    Message, PreviewTypingMessage, RecordingStopModeMessage, WriteMethodMessage,
};
use cosmic::prelude::*;

impl AppModel {
    /// Handle preview typing messages
    pub(in crate::core::app) fn handle_preview_typing_messages(
        &mut self,
        message: PreviewTypingMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Confirm-then-apply: the local value is set only when the daemon
            // acks the save (via `PreviewTypingSettingLoaded`). A failed POST
            // therefore leaves the toggle on its old, correct value instead of
            // stranding an un-rolled-back optimistic one (Tier 1 #15).
            PreviewTypingMessage::Toggled(enabled) => {
                Task::perform(set_preview_typing(enabled), move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::PreviewTyping(
                        PreviewTypingMessage::SettingLoaded(enabled),
                    )),
                    Err(e) => cosmic::Action::App(Message::PreviewTyping(
                        PreviewTypingMessage::Error(e.to_string()),
                    )),
                })
            }

            PreviewTypingMessage::SettingLoaded(enabled) => {
                self.preview_typing_enabled = enabled;
                self.clear_action_error(crate::state::ErrorScope::Recording);
                Task::none()
            }

            PreviewTypingMessage::Error(err) => {
                // The toggle already reflects the daemon's last-known value (we
                // never applied optimistically), so there's nothing to roll back
                // — surface the failure on the Recording page's banner instead of
                // only logging it (Tier 3 #11).
                log::warn!("Preview typing error: {err}");
                self.set_action_error(
                    crate::state::ErrorScope::Recording,
                    format!("Couldn't save preview typing: {err}"),
                );
                Task::none()
            }
        }
    }

    /// Handle recording stop mode messages
    pub(in crate::core::app) fn handle_recording_stop_mode_messages(
        &mut self,
        message: RecordingStopModeMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Confirm-then-apply (see `PreviewTypingToggled`): apply only on the
            // daemon ack so a failed save doesn't strand an optimistic value.
            RecordingStopModeMessage::Changed(mode) => {
                let mode_str = mode.to_string();
                Task::perform(
                    set_recording_stop_mode(mode_str),
                    move |result| match result {
                        Ok(()) => cosmic::Action::App(Message::RecordingStopMode(
                            RecordingStopModeMessage::Loaded(mode),
                        )),
                        Err(e) => cosmic::Action::App(Message::RecordingStopMode(
                            RecordingStopModeMessage::Error(e.to_string()),
                        )),
                    },
                )
            }

            RecordingStopModeMessage::Loaded(mode) => {
                self.recording_stop_mode = mode;
                self.clear_action_error(crate::state::ErrorScope::Recording);
                Task::none()
            }

            RecordingStopModeMessage::Error(err) => {
                log::warn!("Recording stop mode error: {err}");
                self.set_action_error(
                    crate::state::ErrorScope::Recording,
                    format!("Couldn't save recording stop mode: {err}"),
                );
                Task::none()
            }
        }
    }

    /// Handle write method messages
    pub(in crate::core::app) fn handle_write_method_messages(
        &mut self,
        message: WriteMethodMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Confirm-then-apply (see `PreviewTypingToggled`): apply only on the
            // daemon ack so a failed save doesn't strand an optimistic value.
            WriteMethodMessage::Changed(method) => {
                let method_str = method.to_string();
                Task::perform(set_write_method(method_str), move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::WriteMethod(
                        WriteMethodMessage::Loaded(method),
                    )),
                    Err(e) => cosmic::Action::App(Message::WriteMethod(WriteMethodMessage::Error(
                        e.to_string(),
                    ))),
                })
            }

            WriteMethodMessage::Loaded(method) => {
                self.write_method = method;
                self.clear_action_error(crate::state::ErrorScope::InputSimulation);
                Task::none()
            }

            WriteMethodMessage::Error(err) => {
                log::warn!("Write method error: {err}");
                self.set_action_error(
                    crate::state::ErrorScope::InputSimulation,
                    format!("Couldn't save write method: {err}"),
                );
                Task::none()
            }
        }
    }
}
