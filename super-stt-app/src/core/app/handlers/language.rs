// SPDX-License-Identifier: GPL-3.0-only
use crate::core::app::AppModel;
use crate::daemon::client::v1::pipeline::language as model_lang;
use crate::daemon::client::v1::settings::language as client;
use crate::state::models::ContextPage;
use crate::ui::messages::{DaemonMessage, LanguageMessage, Message};
use cosmic::prelude::*;

impl AppModel {
    pub(in crate::core::app) fn handle_language_messages(
        &mut self,
        message: LanguageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            LanguageMessage::OpenLanguagePicker { model } => {
                self.language.language_picker_target = model;
                self.language.language_picker_query.clear();
                self.context_page = ContextPage::LanguagePicker;
                self.core.window.show_context = true;
                Task::none()
            }
            LanguageMessage::CloseLanguagePicker => {
                self.core.window.show_context = false;
                Task::none()
            }
            LanguageMessage::LanguagePickerQueryChanged(q) => {
                self.language.language_picker_query = q;
                Task::none()
            }
            LanguageMessage::PrimaryLanguageLoaded(lang) => {
                self.language.primary_language = lang;
                Task::none()
            }
            LanguageMessage::PrimaryLanguageSelected(choice) => {
                self.language.primary_language.clone_from(&choice);
                self.core.window.show_context = false;
                Task::perform(
                    async move {
                        match choice {
                            Some(tag) => client::set_primary_language(tag).await,
                            None => client::clear_primary_language().await,
                        }
                    },
                    |res| match res {
                        Ok(()) => {
                            cosmic::Action::App(Message::Daemon(DaemonMessage::RefreshDaemonStatus))
                        }
                        Err(e) => cosmic::Action::App(Message::Language(
                            LanguageMessage::LanguageError(e.to_string()),
                        )),
                    },
                )
            }
            per_model @ (LanguageMessage::ModelLanguagesLoaded { .. }
            | LanguageMessage::ModelLanguageLoaded { .. }
            | LanguageMessage::ModelLanguageSelected { .. }) => {
                self.handle_model_language_messages(per_model)
            }
            LanguageMessage::LanguageError(e) => {
                // The language picker lives on the Customization page — surface
                // the failure on that page's banner instead of only logging it
                // (Tier 3 #11).
                log::warn!("Language settings error: {e}");
                self.set_action_error(
                    crate::state::ErrorScope::Customization,
                    format!("Couldn't update language: {e}"),
                );
                Task::none()
            }
        }
    }

    /// The arms that act on one model's language, split out so the dispatch
    /// above stays a table of the global settings.
    fn handle_model_language_messages(
        &mut self,
        message: LanguageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            LanguageMessage::ModelLanguagesLoaded {
                stage,
                source,
                model,
                languages,
            } => {
                self.language
                    .model_languages
                    .record_offer(stage, source, model, languages);
                Task::none()
            }
            LanguageMessage::ModelLanguageLoaded {
                stage,
                source,
                model,
                block,
            } => {
                self.language
                    .model_languages
                    .record(stage, source, model, block);
                Task::none()
            }
            LanguageMessage::ModelLanguageSelected {
                stage,
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
                            Some(tag) => model_lang::set_model_language(stage, model, tag).await,
                            None => model_lang::clear_model_language(stage, model).await,
                        }
                    },
                    move |res| match res {
                        Ok(block) => cosmic::Action::App(Message::Language(
                            LanguageMessage::ModelLanguageLoaded {
                                stage,
                                source: src.clone(),
                                model: mdl.clone(),
                                block,
                            },
                        )),
                        Err(e) => cosmic::Action::App(Message::Language(
                            LanguageMessage::LanguageError(e.to_string()),
                        )),
                    },
                )
            }
            other => {
                unreachable!("handle_model_language_messages received {other:?}")
            }
        }
    }

    /// Fetch the global Primary Language from the daemon (call on connect).
    #[allow(clippy::unused_self)]
    pub(in crate::core::app) fn load_primary_language(&self) -> Task<cosmic::Action<Message>> {
        Task::perform(client::get_primary_language(), |res| match res {
            Ok(lang) => cosmic::Action::App(Message::Language(
                LanguageMessage::PrimaryLanguageLoaded(lang),
            )),
            Err(e) => cosmic::Action::App(Message::Language(LanguageMessage::LanguageError(
                e.to_string(),
            ))),
        })
    }

    /// Fetch a specific model's language resolution block from the daemon.
    /// Call when a model becomes selected (staged or loaded) so the
    /// active-backend card can populate its language control pre-load.
    #[allow(clippy::unused_self)]
    pub(in crate::core::app) fn load_model_language(
        &self,
        stage: u32,
        source: &str,
        model: String,
    ) -> Task<cosmic::Action<Message>> {
        // An idle stage has no model to ask about. Callers report "nothing
        // loaded" as an empty pair rather than an absent one, so without this a
        // daemon that comes up idle answers `400 invalid_backend` and the
        // Customization page shows "Couldn't update language" for a language
        // nobody touched.
        if model.is_empty() || source.is_empty() {
            return Task::none();
        }
        let src = source.to_string();
        let mdl = model.clone();
        let block = Task::perform(
            model_lang::get_model_language(stage, model.clone()),
            move |res| match res {
                Ok(block) => {
                    cosmic::Action::App(Message::Language(LanguageMessage::ModelLanguageLoaded {
                        stage,
                        source: src.clone(),
                        model: mdl.clone(),
                        block,
                    }))
                }
                Err(e) => cosmic::Action::App(Message::Language(LanguageMessage::LanguageError(
                    e.to_string(),
                ))),
            },
        );
        Task::batch([block, Self::load_model_languages(stage, source, model)])
    }

    /// Fetch what a model can be pinned to, so the picker offers exactly what
    /// the daemon will accept.
    ///
    /// Paired with the block above rather than folded into it: they are
    /// separate endpoints because they answer separate questions, and only one
    /// of them changes when the user picks a language.
    fn load_model_languages(
        stage: u32,
        source: &str,
        model: String,
    ) -> Task<cosmic::Action<Message>> {
        let src = source.to_string();
        let mdl = model.clone();
        Task::perform(
            model_lang::list_model_languages(stage, model),
            move |res| match res {
                Ok(languages) => {
                    cosmic::Action::App(Message::Language(LanguageMessage::ModelLanguagesLoaded {
                        stage,
                        source: src.clone(),
                        model: mdl.clone(),
                        languages,
                    }))
                }
                // A failed read leaves the picker with nothing to offer rather
                // than a general language list the model would refuse.
                Err(e) => {
                    log::warn!("Could not read the languages for {mdl}: {e}");
                    cosmic::Action::None
                }
            },
        )
    }
}
