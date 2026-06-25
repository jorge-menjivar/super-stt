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
            Message::OpenLanguagePicker { model } => {
                self.language_picker_target = model;
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
            Message::ModelLanguageLoaded {
                source,
                model,
                block,
            } => {
                self.model_language = Some(block);
                self.model_language_for = Some((source, model));
                Task::none()
            }
            Message::ModelLanguageSelected {
                source,
                model,
                choice,
            } => {
                self.core.window.show_context = false;
                let src = source.clone();
                let mdl = model.clone();
                Task::perform(
                    async move {
                        match choice {
                            Some(tag) => client::set_model_language(source, model, tag).await,
                            None => client::clear_model_language(source, model).await,
                        }
                    },
                    move |res| match res {
                        Ok(block) => cosmic::Action::App(Message::ModelLanguageLoaded {
                            source: src.clone(),
                            model: mdl.clone(),
                            block,
                        }),
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

    /// Fetch a specific model's language resolution block from the daemon.
    /// Call when a model becomes selected (staged or loaded) so the
    /// active-backend card can populate its language control pre-load.
    #[allow(clippy::unused_self)]
    pub(in crate::core::app) fn load_model_language(
        &self,
        source: String,
        model: String,
    ) -> Task<cosmic::Action<Message>> {
        let src = source.clone();
        let mdl = model.clone();
        Task::perform(
            client::get_model_language(source, model),
            move |res| match res {
                Ok(block) => cosmic::Action::App(Message::ModelLanguageLoaded {
                    source: src.clone(),
                    model: mdl.clone(),
                    block,
                }),
                Err(e) => cosmic::Action::App(Message::LanguageError(e)),
            },
        )
    }
}
