// SPDX-License-Identifier: GPL-3.0-only
use crate::core::app::AppModel;
// Two language surfaces, two modules. The global setting is a daemon-wide
// preference under `/settings`; a model's override is addressed through the
// stage that resolves its bare name to a backend, and so lives under
// `/pipeline`. They used to share one alias here, which is exactly the sort of
// thing that survives a move by pointing at whichever module still happens to
// export a matching name.
use crate::daemon::client::v1::pipeline::language as model_language;
use crate::daemon::client::v1::settings::language as global_language;
use crate::state::models::ContextPage;
use crate::ui::messages::{DaemonMessage, LanguageMessage, Message};
use cosmic::prelude::*;
use super_tts_shared::models::protocol::SYNTHESIS_STAGE;

impl AppModel {
    /// Route every language message to the half that owns it. Split in two
    /// because the sheet's own state (which model it targets, what has been
    /// typed into its search box) has nothing to do with the values the daemon
    /// holds, and folding them together made one function nobody could read end
    /// to end.
    pub(in crate::core::app) fn handle_language_messages(
        &mut self,
        message: LanguageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            LanguageMessage::OpenLanguagePicker { .. }
            | LanguageMessage::CloseLanguagePicker
            | LanguageMessage::LanguagePickerQueryChanged(_) => self.handle_language_sheet(message),

            LanguageMessage::PrimaryLanguageLoaded(_)
            | LanguageMessage::PrimaryLanguagesListed(_)
            | LanguageMessage::PrimaryLanguageSelected(_)
            | LanguageMessage::ModelLanguageLoaded { .. }
            | LanguageMessage::ModelLanguagesListed { .. }
            | LanguageMessage::ModelLanguageSelected { .. }
            | LanguageMessage::LanguageError(_) => self.handle_language_values(message),
        }
    }

    /// The search sheet itself: which language it is choosing for, and what has
    /// been typed into it.
    fn handle_language_sheet(&mut self, message: LanguageMessage) -> Task<cosmic::Action<Message>> {
        match message {
            LanguageMessage::OpenLanguagePicker { model } => {
                self.language.language_picker_target.clone_from(&model);
                self.language.language_picker_query.clear();
                self.context_page = ContextPage::LanguagePicker;
                self.core.window.show_context = true;
                // Re-read what the sheet is about to offer. Both lists are the
                // daemon's, and both change as backends are installed and
                // removed — the global one because it is the union of what the
                // installed models can speak. Opening the sheet is the one
                // moment the answer is actually needed, so it is the moment to
                // ask; the previous answer stays on screen until this lands, so
                // there is no empty frame.
                match model {
                    Some((source, model)) => self.list_model_languages(&source, model),
                    None => Self::list_primary_languages(),
                }
            }
            LanguageMessage::CloseLanguagePicker => {
                self.core.window.show_context = false;
                Task::none()
            }
            LanguageMessage::LanguagePickerQueryChanged(q) => {
                self.language.language_picker_query = q;
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// The values themselves: what the daemon holds, what it will accept, and
    /// what the user chose.
    fn handle_language_values(
        &mut self,
        message: LanguageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            LanguageMessage::PrimaryLanguageLoaded(lang) => {
                self.language.primary_language = lang;
                Task::none()
            }
            LanguageMessage::PrimaryLanguagesListed(languages) => {
                self.language.primary_languages = languages;
                Task::none()
            }
            LanguageMessage::ModelLanguagesListed {
                source,
                model,
                languages,
            } => {
                // Late answers happen: staging a second model while the first
                // model's list is still in flight. Apply it only if it is still
                // the model the rest of the language state describes, or the
                // picker offers one model's languages under another's name.
                if self.language.model_language_for.as_ref() == Some(&(source, model)) {
                    self.language.model_languages = languages;
                }
                Task::none()
            }
            LanguageMessage::PrimaryLanguageSelected(choice) => {
                self.language.primary_language.clone_from(&choice);
                self.core.window.show_context = false;
                Task::perform(
                    async move {
                        match choice {
                            Some(tag) => global_language::set_primary_language(tag).await,
                            None => global_language::clear_primary_language().await,
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
            LanguageMessage::ModelLanguageLoaded {
                source,
                model,
                block,
            } => {
                // This is where the pair moves, and the offered list beside it
                // belongs to the model it just moved off. Drop that list and go
                // ask for this model's — after the pair is set, not alongside
                // it: issued as a pair of concurrent reads, whichever landed
                // first would be judged against a pair the other had not
                // established yet, and the list would be discarded as stale on
                // the very first fetch for every model.
                //
                // A pick lands here too, carrying the resolution the daemon
                // returned for the same pair. Nothing to re-ask there: choosing
                // a language cannot change which languages are on offer.
                let pair = Some((source.clone(), model.clone()));
                let model_changed = self.language.model_language_for != pair;
                self.language.model_language = Some(block);
                self.language.model_language_for = pair;
                if model_changed {
                    self.language.model_languages.clear();
                    return self.list_model_languages(&source, model);
                }
                Task::none()
            }
            LanguageMessage::ModelLanguageSelected {
                source,
                model,
                choice,
            } => {
                self.core.window.show_context = false;
                let src = source.clone();
                let mdl = model.clone();
                // `source` is not sent: the stage resolves the bare model name
                // against the backend filling it, and guessing the source by
                // scanning for whoever serves the name is how one backend's
                // preference gets written onto another's model. It is kept here
                // purely as the key the resulting block is filed under.
                Task::perform(
                    async move {
                        match choice {
                            Some(tag) => {
                                model_language::set_model_language(SYNTHESIS_STAGE, model, tag)
                                    .await
                            }
                            None => {
                                model_language::clear_model_language(SYNTHESIS_STAGE, model).await
                            }
                        }
                    },
                    move |res| match res {
                        Ok(block) => cosmic::Action::App(Message::Language(
                            LanguageMessage::ModelLanguageLoaded {
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

            _ => Task::none(),
        }
    }

    /// Fetch the global Primary Language and the tags it accepts (call on
    /// connect).
    ///
    /// Both, because a value with nothing to change it to is a dead control:
    /// the picker's contents are the daemon's answer, and reading the setting
    /// without reading the vocabulary leaves the sheet empty until the user
    /// opens it a second time.
    #[allow(clippy::unused_self)]
    pub(in crate::core::app) fn load_primary_language(&self) -> Task<cosmic::Action<Message>> {
        Task::batch([
            Task::perform(global_language::get_primary_language(), |res| match res {
                Ok(lang) => cosmic::Action::App(Message::Language(
                    LanguageMessage::PrimaryLanguageLoaded(lang),
                )),
                Err(e) => cosmic::Action::App(Message::Language(LanguageMessage::LanguageError(
                    e.to_string(),
                ))),
            }),
            Self::list_primary_languages(),
        ])
    }

    /// Re-read the tags the global Primary Language setting accepts.
    ///
    /// Separate from the value above so the picker can refresh its options on
    /// open without also re-reading a setting the app already holds — and
    /// because the list, unlike the value, goes stale on its own: installing a
    /// backend adds every language its models speak.
    ///
    /// A read failure is reported rather than swallowed. Silently keeping the
    /// previous list would leave a picker offering languages of a backend that
    /// has since been removed, with nothing on screen to say the list is old.
    fn list_primary_languages() -> Task<cosmic::Action<Message>> {
        Task::perform(global_language::list_primary_languages(), |res| match res {
            Ok(languages) => cosmic::Action::App(Message::Language(
                LanguageMessage::PrimaryLanguagesListed(languages),
            )),
            Err(e) => cosmic::Action::App(Message::Language(LanguageMessage::LanguageError(
                e.to_string(),
            ))),
        })
    }

    /// Fetch a specific model's language resolution from the daemon. Call when a
    /// model becomes selected (staged or loaded) so the active-backend card can
    /// populate its language control pre-load.
    ///
    /// Only the resolution. The tags the model can be *pinned* to are a separate
    /// read, chained from this one's handler once the pair it describes is
    /// established — see [`Self::list_model_languages`]. That set used to ride
    /// along inside the block and no longer does, which is why a card that stops
    /// here renders its picker with nothing in it.
    #[allow(clippy::unused_self)]
    pub(in crate::core::app) fn load_model_language(
        &self,
        source: &str,
        model: String,
    ) -> Task<cosmic::Action<Message>> {
        let src = source.to_string();
        let mdl = model.clone();
        Task::perform(
            model_language::get_model_language(SYNTHESIS_STAGE, model),
            move |res| match res {
                Ok(block) => {
                    cosmic::Action::App(Message::Language(LanguageMessage::ModelLanguageLoaded {
                        source: src.clone(),
                        model: mdl.clone(),
                        block,
                    }))
                }
                Err(e) => cosmic::Action::App(Message::Language(LanguageMessage::LanguageError(
                    e.to_string(),
                ))),
            },
        )
    }

    /// Re-read the tags one model can be pinned to.
    ///
    /// Split from the resolution for the same reason the daemon splits the
    /// endpoints: picking a language rewrites the resolution and leaves this
    /// list exactly as it was, so the picker refreshes its options on open
    /// without re-requesting an answer that cannot have changed.
    ///
    /// Called only once the held `(source, model)` pair already names this
    /// model — from the resolution's own handler, and from opening the sheet.
    /// The answer is checked against that pair on arrival, so issuing it before
    /// the pair is set would have it discarded as another model's.
    #[allow(clippy::unused_self)]
    fn list_model_languages(&self, source: &str, model: String) -> Task<cosmic::Action<Message>> {
        let src = source.to_string();
        let mdl = model.clone();
        Task::perform(
            model_language::list_model_languages(SYNTHESIS_STAGE, model),
            move |res| match res {
                Ok(languages) => {
                    cosmic::Action::App(Message::Language(LanguageMessage::ModelLanguagesListed {
                        source: src.clone(),
                        model: mdl.clone(),
                        languages,
                    }))
                }
                Err(e) => cosmic::Action::App(Message::Language(LanguageMessage::LanguageError(
                    e.to_string(),
                ))),
            },
        )
    }
}
