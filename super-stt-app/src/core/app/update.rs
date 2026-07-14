// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::ui::messages::Message;
use cosmic::prelude::*;

impl AppModel {
    /// Pure message dispatcher — routes every [`Message`] group to its handler
    /// and returns the resulting [`Task`]. The `match` is exhaustive: a newly
    /// added sub-enum forces a new arm here, and each handler `match`es its
    /// sub-enum exhaustively, so a forgotten variant is a compile error rather
    /// than a silent no-op.
    pub(in crate::core::app) fn dispatch(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::Shell(m) => self.handle_shell_messages(m),
            Message::Daemon(m) => self.handle_daemon_messages(m),
            Message::Model(m) => self.handle_model_messages(m),
            Message::ModelsPage(m) => self.handle_models_page_messages(m),
            Message::Device(m) => self.handle_device_messages(m),
            Message::Download(m) => self.handle_download_messages(m),
            Message::PreviewTyping(m) => self.handle_preview_typing_messages(m),
            Message::RecordingStopMode(m) => self.handle_recording_stop_mode_messages(m),
            Message::WriteMethod(m) => self.handle_write_method_messages(m),
            Message::Backend(m) => self.handle_backend_messages(m),
            Message::Language(m) => self.handle_language_messages(m),
            Message::Recording(m) => self.handle_recording_messages(m),

            // Scoped action failure: park it in the per-page banner slot.
            Message::SettingActionFailed { scope, message } => {
                self.action_error = Some(crate::state::ActionError { scope, message });
                Task::none()
            }
        }
    }
}
