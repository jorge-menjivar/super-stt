// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::{
    set_notification_method, set_preview_typing, set_recording_stop_mode, set_write_method,
    test_write_method,
};
use crate::ui::messages::{
    Message, NotificationMethodMessage, PreviewTypingMessage, RecordingStopModeMessage,
    WriteMethodMessage,
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
                        format!("couldn't save: {e}"),
                    ))),
                })
            }

            WriteMethodMessage::Loaded(method) => {
                self.write_method = method;
                // The stored resolution belonged to the previous method.
                self.resolved_write_method = None;
                self.clear_action_error(crate::state::ErrorScope::InputSimulation);
                Task::none()
            }

            // Focus the test field *before* asking the daemon to type: it types
            // into whatever window holds focus, and pressing the button leaves
            // focus on the button. `chain` orders the two, where `batch` would
            // race the focus against the round-trip.
            WriteMethodMessage::Test => {
                self.write_method_test_text.clear();
                self.clear_action_error(crate::state::ErrorScope::InputSimulation);
                cosmic::widget::text_input::focus(
                    crate::ui::views::input_simulation::test_field_id(),
                )
                .chain(Task::perform(test_write_method(), |result| match result {
                    // `None` is a daemon that typed but named no backend this
                    // build knows: the test still passed, so report it and
                    // leave the backend readout empty rather than guessing.
                    Ok(resolved) => cosmic::Action::App(Message::WriteMethod(
                        WriteMethodMessage::Tested(resolved),
                    )),
                    Err(e) => cosmic::Action::App(Message::WriteMethod(WriteMethodMessage::Error(
                        format!("test failed: {e}"),
                    ))),
                }))
            }

            WriteMethodMessage::Tested(resolved) => {
                self.resolved_write_method = resolved;
                Task::none()
            }

            WriteMethodMessage::TestInput(text) => {
                self.write_method_test_text = text;
                Task::none()
            }

            WriteMethodMessage::Error(err) => {
                log::warn!("Write method error: {err}");
                self.set_action_error(
                    crate::state::ErrorScope::InputSimulation,
                    format!("Write method: {err}"),
                );
                Task::none()
            }
        }
    }

    /// Handle notification method messages
    pub(in crate::core::app) fn handle_notification_method_messages(
        &mut self,
        message: NotificationMethodMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Confirm-then-apply, as with the write method: apply only on the
            // daemon ack so a failed save doesn't strand an optimistic value.
            NotificationMethodMessage::Changed(method) => {
                let method_str = method.to_string();
                Task::perform(
                    set_notification_method(method_str),
                    move |result| match result {
                        Ok(()) => cosmic::Action::App(Message::NotificationMethod(
                            NotificationMethodMessage::Loaded(method),
                        )),
                        Err(e) => cosmic::Action::App(Message::NotificationMethod(
                            NotificationMethodMessage::Error(e.to_string()),
                        )),
                    },
                )
            }

            NotificationMethodMessage::Loaded(method) => {
                self.notification_method = method;
                self.clear_action_error(crate::state::ErrorScope::Recording);
                Task::none()
            }

            NotificationMethodMessage::Error(err) => {
                log::warn!("Notification method error: {err}");
                self.set_action_error(
                    crate::state::ErrorScope::Recording,
                    format!("Couldn't save notification method: {err}"),
                );
                Task::none()
            }
        }
    }
}
