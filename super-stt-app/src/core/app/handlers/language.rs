// SPDX-License-Identifier: GPL-3.0-only
use crate::core::app::AppModel;
use crate::daemon::client::v1::settings::language as client;
use crate::state::models::ContextPage;
use crate::ui::messages::Message;
use cosmic::prelude::*;

impl AppModel {
    pub(in crate::core::app) fn handle_language_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::OpenLanguagePicker { per_model } => {
                self.language_picker_per_model = per_model;
                self.language_picker_query.clear();
                self.context_page = ContextPage::LanguagePicker;
                self.core.window.show_context = true;
                Task::none()
            }
            Message::CloseLanguagePicker => {
                self.core.window.show_context = false;
                Task::none()
            }
            Message::LanguagePickerQueryChanged(q) => {
                self.language_picker_query = q;
                Task::none()
            }
            Message::PrimaryLanguageLoaded(lang) => {
                self.primary_language = lang;
                Task::none()
            }
            Message::PrimaryLanguageSelected(choice) => {
                self.primary_language.clone_from(&choice);
                self.core.window.show_context = false;
                Task::perform(
                    async move {
                        match choice {
                            Some(tag) => client::set_primary_language(tag).await,
                            None => client::clear_primary_language().await,
                        }
                    },
                    |res| match res {
                        Ok(()) => cosmic::Action::App(Message::RefreshDaemonStatus),
                        Err(e) => cosmic::Action::App(Message::LanguageError(e)),
                    },
                )
            }
            Message::ActiveModelLanguageLoaded(block) => {
                self.active_model_language = Some(block);
                Task::none()
            }
            Message::ActiveModelLanguageSelected(choice) => {
                self.core.window.show_context = false;
                Task::perform(
                    async move {
                        match choice {
                            Some(tag) => client::set_active_model_language(tag).await,
                            None => client::clear_active_model_language().await,
                        }
                    },
                    |res| match res {
                        Ok(block) => cosmic::Action::App(Message::ActiveModelLanguageLoaded(block)),
                        Err(e) => cosmic::Action::App(Message::LanguageError(e)),
                    },
                )
            }
            Message::LanguageError(e) => {
                log::warn!("Language settings error: {e}");
                Task::none()
            }
            _ => Task::none(),
        }
    }

    /// Fetch the global Primary Language from the daemon (call on connect).
    #[allow(clippy::unused_self)]
    pub(in crate::core::app) fn load_primary_language(&self) -> Task<cosmic::Action<Message>> {
        Task::perform(client::get_primary_language(), |res| match res {
            Ok(lang) => cosmic::Action::App(Message::PrimaryLanguageLoaded(lang)),
            Err(e) => cosmic::Action::App(Message::LanguageError(e)),
        })
    }

    /// Fetch the active model's language resolution (call when a model becomes active).
    #[allow(clippy::unused_self)]
    pub(in crate::core::app) fn load_active_model_language(&self) -> Task<cosmic::Action<Message>> {
        Task::perform(client::get_active_model_language(), |res| match res {
            Ok(block) => cosmic::Action::App(Message::ActiveModelLanguageLoaded(block)),
            Err(e) => cosmic::Action::App(Message::LanguageError(e)),
        })
    }
}
