// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::set_notification_method;
use crate::ui::messages::{Message, NotificationMethodMessage};
use cosmic::prelude::*;

impl AppModel {
    /// Handle notification method messages
    pub(in crate::core::app) fn handle_notification_method_messages(
        &mut self,
        message: NotificationMethodMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Confirm-then-apply: the dropdown moves only on the daemon's ack,
            // so a failed save doesn't strand the UI on a value the daemon
            // rejected.
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
                self.clear_action_error(crate::state::ErrorScope::Speech);
                Task::none()
            }

            NotificationMethodMessage::Error(err) => {
                log::warn!("Notification method error: {err}");
                self.set_action_error(
                    crate::state::ErrorScope::Speech,
                    format!("Couldn't save notification method: {err}"),
                );
                Task::none()
            }
        }
    }
}
