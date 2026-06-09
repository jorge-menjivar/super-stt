// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::{
    list_available_models, list_backends, set_custom_models_dir, set_preview_typing,
    set_recording_stop_mode, set_write_method,
};
use crate::ui::messages::Message;
use cosmic::prelude::*;

impl AppModel {
    /// Handle preview typing messages
    pub(in crate::core::app) fn handle_preview_typing_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::PreviewTypingToggled(enabled) => {
                self.preview_typing_enabled = enabled;
                Task::perform(set_preview_typing(enabled), move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::PreviewTypingSettingLoaded(enabled)),
                    Err(e) => cosmic::Action::App(Message::PreviewTypingError(e)),
                })
            }

            Message::PreviewTypingSettingLoaded(enabled) => {
                self.preview_typing_enabled = enabled;
                Task::none()
            }

            Message::PreviewTypingError(err) => {
                // Log error and show it to user in transcription text
                log::warn!("Preview typing error: {err}");
                self.transcription_text = format!("Preview Typing Error: {err}");
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
            Message::RecordingStopModeChanged(mode) => {
                self.recording_stop_mode = mode;
                let mode_str = mode.to_string();
                Task::perform(
                    set_recording_stop_mode(mode_str),
                    move |result| match result {
                        Ok(()) => cosmic::Action::App(Message::RecordingStopModeLoaded(mode)),
                        Err(e) => cosmic::Action::App(Message::RecordingStopModeError(e)),
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
            Message::WriteMethodChanged(method) => {
                self.write_method = method;
                let method_str = method.to_string();
                Task::perform(set_write_method(method_str), move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::WriteMethodLoaded(method)),
                    Err(e) => cosmic::Action::App(Message::WriteMethodError(e)),
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

    /// Handle custom models directory messages
    pub(in crate::core::app) fn handle_custom_models_dir_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::CustomModelsDirInput(input) => {
                self.custom_models_dir_input = input;
                Task::none()
            }

            Message::CustomModelsDirEdit(editing) => {
                self.custom_models_dir_editing = editing;
                if editing {
                    self.custom_models_dir_input =
                        self.custom_models_dir.clone().unwrap_or_default();
                }
                Task::none()
            }

            Message::CustomModelsDirSet(path) => {
                self.custom_models_dir_input = path.as_deref().unwrap_or_default().to_string();
                self.custom_models_dir_editing = false;
                self.custom_models_dir.clone_from(&path);
                Task::batch([
                    Task::perform(
                        async move {
                            set_custom_models_dir(path).await?;
                            // Re-fetch model list so newly discovered custom models appear
                            list_available_models().await
                        },
                        |result| match result {
                            Ok(models) => {
                                cosmic::Action::App(Message::AvailableModelsLoaded(models))
                            }
                            Err(e) => cosmic::Action::App(Message::CustomModelsDirError(e)),
                        },
                    ),
                    // The backend catalog also changes when the custom
                    // models dir changes (newly discovered local models).
                    Task::perform(list_backends(), |result| match result {
                        Ok(backends) => cosmic::Action::App(Message::BackendsLoaded(backends)),
                        Err(e) => cosmic::Action::App(Message::BackendsError(e)),
                    }),
                ])
            }

            Message::CustomModelsDirError(err) => {
                log::warn!("Custom models dir error: {err}");
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
