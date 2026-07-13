// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::{set_preview_typing, set_recording_stop_mode, set_write_method};
use crate::ui::messages::Message;
use cosmic::prelude::*;

impl AppModel {
    /// Handle preview typing messages
    pub(in crate::core::app) fn handle_preview_typing_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Confirm-then-apply: the local value is set only when the daemon
            // acks the save (via `PreviewTypingSettingLoaded`). A failed POST
            // therefore leaves the toggle on its old, correct value instead of
            // stranding an un-rolled-back optimistic one (Tier 1 #15).
            Message::PreviewTypingToggled(enabled) => {
                Task::perform(set_preview_typing(enabled), move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::PreviewTypingSettingLoaded(enabled)),
                    Err(e) => cosmic::Action::App(Message::PreviewTypingError(e.to_string())),
                })
            }

            Message::PreviewTypingSettingLoaded(enabled) => {
                self.preview_typing_enabled = enabled;
                Task::none()
            }

            Message::PreviewTypingError(err) => {
                // The toggle already reflects the daemon's last-known value (we
                // never applied optimistically), so just log — no transcription
                // box abuse.
                log::warn!("Preview typing error: {err}");
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle recording stop mode messages
    pub(in crate::core::app) fn handle_recording_stop_mode_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Confirm-then-apply (see `PreviewTypingToggled`): apply only on the
            // daemon ack so a failed save doesn't strand an optimistic value.
            Message::RecordingStopModeChanged(mode) => {
                let mode_str = mode.to_string();
                Task::perform(
                    set_recording_stop_mode(mode_str),
                    move |result| match result {
                        Ok(()) => cosmic::Action::App(Message::RecordingStopModeLoaded(mode)),
                        Err(e) => {
                            cosmic::Action::App(Message::RecordingStopModeError(e.to_string()))
                        }
                    },
                )
            }

            Message::RecordingStopModeLoaded(mode) => {
                self.recording_stop_mode = mode;
                Task::none()
            }

            Message::RecordingStopModeError(err) => {
                log::warn!("Recording stop mode error: {err}");
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle write method messages
    pub(in crate::core::app) fn handle_write_method_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Confirm-then-apply (see `PreviewTypingToggled`): apply only on the
            // daemon ack so a failed save doesn't strand an optimistic value.
            Message::WriteMethodChanged(method) => {
                let method_str = method.to_string();
                Task::perform(set_write_method(method_str), move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::WriteMethodLoaded(method)),
                    Err(e) => cosmic::Action::App(Message::WriteMethodError(e.to_string())),
                })
            }

            Message::WriteMethodLoaded(method) => {
                self.write_method = method;
                Task::none()
            }

            Message::WriteMethodError(err) => {
                log::warn!("Write method error: {err}");
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
