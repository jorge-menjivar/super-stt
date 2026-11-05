// SPDX-License-Identifier: GPL-3.0-only
//! The Contexts page: the dictation contexts, and which one is in force.
//!
//! Follows the confirm-then-apply rule (`ui/messages.rs`): nothing in the list
//! moves until the daemon has answered. The editor is the deliberate exception
//! — what the user is typing is local by definition, and a refetch landing
//! under an open editor must not rewrite the sentence they are halfway through.

use crate::core::app::AppModel;
use crate::daemon::client::v1::contexts as client;
use crate::daemon::client::v1::contexts::backend as backend_client;
use crate::state::contexts::ContextDraft;
use crate::state::models::ContextPage;
use crate::ui::messages::{ContextsMessage, Message};
use cosmic::prelude::*;
use cosmic::widget::text_input;

impl AppModel {
    /// Routes to the three groups below. Split only because the one table
    /// outgrew a screen: the groups are the page's three surfaces — the list,
    /// the editor over it, and the picker in a backend's Configure sheet.
    pub(in crate::core::app) fn handle_contexts_messages(
        &mut self,
        message: ContextsMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            list @ (ContextsMessage::Reload
            | ContextsMessage::Loaded { .. }
            | ContextsMessage::LoadFailed(_)
            | ContextsMessage::Activate(_)
            | ContextsMessage::DeleteRequested(_)
            | ContextsMessage::DeleteCancelled
            | ContextsMessage::DeleteConfirmed(_)) => self.handle_context_list_messages(list),

            editor @ (ContextsMessage::Edit(_)
            | ContextsMessage::Create
            | ContextsMessage::CancelEdit
            | ContextsMessage::NameChanged(_)
            | ContextsMessage::PromptAction(_)
            | ContextsMessage::TermChanged { .. }
            | ContextsMessage::TermSubmitted(_)
            | ContextsMessage::TermRemoved(_)
            | ContextsMessage::Save
            | ContextsMessage::Saved(_)
            | ContextsMessage::SaveFailed(_)) => self.handle_context_editor_messages(editor),

            per_backend @ (ContextsMessage::BackendPinned { .. }
            | ContextsMessage::BackendFollowsActive(_)
            | ContextsMessage::BackendContextLoaded { .. }) => {
                self.handle_backend_context_messages(per_backend)
            }
        }
    }

    /// The list itself: fetching it, choosing what is in force, and deleting.
    fn handle_context_list_messages(
        &mut self,
        message: ContextsMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            ContextsMessage::Reload => {
                self.contexts.loading = true;
                Task::perform(client::list_contexts(), |res| match res {
                    Ok(catalog) => app(ContextsMessage::Loaded {
                        active: catalog.active,
                        contexts: catalog.contexts,
                    }),
                    Err(e) => app(ContextsMessage::LoadFailed(e.to_string())),
                })
            }
            ContextsMessage::Loaded { active, contexts } => {
                self.contexts.replace(contexts, active);
                Task::none()
            }
            ContextsMessage::LoadFailed(e) => {
                log::warn!("Contexts could not be loaded: {e}");
                self.contexts.loading = false;
                // Marked loaded even on failure, so the page shows the banner
                // rather than a spinner that never stops.
                self.contexts.loaded = true;
                self.set_action_error(
                    crate::state::ErrorScope::Contexts,
                    format!("Couldn't load contexts: {e}"),
                );
                Task::none()
            }

            ContextsMessage::Activate(id) => {
                self.clear_action_error(crate::state::ErrorScope::Contexts);
                Task::perform(client::set_active_context(id), |res| match res {
                    // The answer carries the new selection, but the list has to
                    // be re-read anyway: activating is the one write whose
                    // effect a *different* row also shows.
                    Ok(_) => app(ContextsMessage::Reload),
                    Err(e) => app(ContextsMessage::LoadFailed(e.to_string())),
                })
            }

            ContextsMessage::DeleteRequested(id) => {
                self.contexts.confirming_delete = Some(id);
                Task::none()
            }
            ContextsMessage::DeleteCancelled => {
                self.contexts.confirming_delete = None;
                Task::none()
            }
            ContextsMessage::DeleteConfirmed(id) => {
                self.contexts.confirming_delete = None;
                self.clear_action_error(crate::state::ErrorScope::Contexts);
                Task::perform(client::delete_context(id), |res| match res {
                    Ok(()) => app(ContextsMessage::Reload),
                    Err(e) => app(ContextsMessage::LoadFailed(e.to_string())),
                })
            }

            // Routed here by the table above; nothing else reaches this arm.
            other => unreachable_group(&other),
        }
    }

    /// The editor sheet over one context.
    fn handle_context_editor_messages(
        &mut self,
        message: ContextsMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            ContextsMessage::Edit(id) => {
                self.contexts.confirming_delete = None;
                if let Some(context) = self.contexts.get(&id) {
                    self.contexts.draft = Some(ContextDraft::editing(context));
                    self.context_page = ContextPage::EditContext;
                    self.core.window.show_context = true;
                }
                Task::none()
            }
            ContextsMessage::Create => {
                self.contexts.draft = Some(ContextDraft::creating(self.contexts.next_id()));
                self.context_page = ContextPage::EditContext;
                self.core.window.show_context = true;
                Task::none()
            }
            ContextsMessage::CancelEdit => {
                self.contexts.draft = None;
                self.core.window.show_context = false;
                Task::none()
            }

            ContextsMessage::NameChanged(name) => {
                if let Some(draft) = self.contexts.draft.as_mut() {
                    draft.name = name;
                    draft.error = None;
                }
                Task::none()
            }
            ContextsMessage::PromptAction(action) => {
                if let Some(draft) = self.contexts.draft.as_mut() {
                    draft.prompt.perform(action);
                    draft.error = None;
                }
                Task::none()
            }
            ContextsMessage::TermChanged { index, value } => {
                if let Some(draft) = self.contexts.draft.as_mut() {
                    draft.set_row(index, &value);
                    draft.error = None;
                }
                Task::none()
            }
            ContextsMessage::TermSubmitted(index) => {
                // Enter opens the next row and puts the cursor in it, so a list
                // is typed rather than clicked together.
                let Some(draft) = self.contexts.draft.as_mut() else {
                    return Task::none();
                };
                let id = draft.insert_row_after(index);
                text_input::focus(id)
            }
            ContextsMessage::TermRemoved(index) => {
                let Some(draft) = self.contexts.draft.as_mut() else {
                    return Task::none();
                };
                draft
                    .remove_row(index)
                    .map_or_else(Task::none, text_input::focus)
            }

            ContextsMessage::Save => {
                let Some(draft) = self.contexts.draft.as_mut() else {
                    return Task::none();
                };
                let context = draft.to_context();
                // Checked here as well as in the daemon, so an empty name is
                // answered by the form rather than by a round trip.
                if let Err(e) = context.check() {
                    draft.error = Some(e);
                    return Task::none();
                }
                draft.saving = true;
                draft.error = None;
                Task::perform(client::set_context(context), |res| match res {
                    Ok(stored) => app(ContextsMessage::Saved(stored)),
                    Err(e) => app(ContextsMessage::SaveFailed(e.to_string())),
                })
            }
            ContextsMessage::Saved(stored) => {
                self.contexts.draft = None;
                self.core.window.show_context = false;
                // Fold in what came back rather than waiting for the refetch,
                // so the row updates in the same frame the sheet closes; the
                // reload that follows is what picks up anything else that moved.
                self.contexts.upsert(stored);
                Task::perform(async {}, |()| app(ContextsMessage::Reload))
            }
            ContextsMessage::SaveFailed(e) => {
                if let Some(draft) = self.contexts.draft.as_mut() {
                    draft.saving = false;
                    draft.error = Some(e);
                } else {
                    // The sheet was closed under the in-flight save; the page
                    // banner is the only place left to say so.
                    self.set_action_error(
                        crate::state::ErrorScope::Contexts,
                        format!("Couldn't save that context: {e}"),
                    );
                }
                Task::none()
            }

            other => unreachable_group(&other),
        }
    }

    /// Which context one backend uses, from its Configure sheet.
    fn handle_backend_context_messages(
        &mut self,
        message: ContextsMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            ContextsMessage::BackendPinned { source, id } => {
                self.clear_action_error(crate::state::ErrorScope::ConfigureBackend);
                let for_message = source.clone();
                Task::perform(
                    backend_client::set_backend_context(source, id),
                    move |res| match res {
                        Ok(state) => app(ContextsMessage::BackendContextLoaded {
                            source: for_message.clone(),
                            state,
                        }),
                        Err(e) => cosmic::Action::App(Message::SettingActionFailed {
                            scope: crate::state::ErrorScope::ConfigureBackend,
                            message: format!("Couldn't change this backend's context: {e}"),
                        }),
                    },
                )
            }
            ContextsMessage::BackendFollowsActive(source) => {
                self.clear_action_error(crate::state::ErrorScope::ConfigureBackend);
                let for_message = source.clone();
                Task::perform(
                    backend_client::clear_backend_context(source),
                    move |res| match res {
                        Ok(state) => app(ContextsMessage::BackendContextLoaded {
                            source: for_message.clone(),
                            state,
                        }),
                        Err(e) => cosmic::Action::App(Message::SettingActionFailed {
                            scope: crate::state::ErrorScope::ConfigureBackend,
                            message: format!("Couldn't change this backend's context: {e}"),
                        }),
                    },
                )
            }
            ContextsMessage::BackendContextLoaded { source, state } => {
                self.contexts.backend_contexts.insert(source, state);
                Task::none()
            }

            other => unreachable_group(&other),
        }
    }

    /// Ask the daemon what one backend uses, for the Configure sheet's picker.
    ///
    /// Read on demand rather than published with the catalog: it is one line of
    /// the sheet, and `GET /backend/list` is fetched on every navigation to two
    /// pages that never show it.
    pub(in crate::core::app) fn fetch_backend_context(
        source: String,
    ) -> Task<cosmic::Action<Message>> {
        let for_message = source.clone();
        Task::perform(
            backend_client::get_backend_context(source),
            move |res| match res {
                Ok(state) => app(ContextsMessage::BackendContextLoaded {
                    source: for_message.clone(),
                    state,
                }),
                // A backend whose context cannot be read renders as "follows
                // the active one", which is the default and the truth for
                // almost every backend. Not worth a banner in a sheet that is
                // about secrets and options.
                Err(e) => {
                    log::warn!("Couldn't read this backend's context: {e}");
                    cosmic::Action::None
                }
            },
        )
    }
}

/// `ContextsMessage` as a cosmic action, since every arm above builds one.
fn app(message: ContextsMessage) -> cosmic::Action<Message> {
    cosmic::Action::App(Message::Contexts(message))
}

/// A message that reached the wrong group.
///
/// The router above is exhaustive, so this is unreachable by construction — but
/// the grouped handlers take the whole enum, so the compiler cannot say so.
/// Logged rather than panicking: a misrouted settings message is not worth
/// taking the window down for.
fn unreachable_group(message: &ContextsMessage) -> Task<cosmic::Action<Message>> {
    log::error!("Contexts message reached the wrong handler: {message:?}");
    Task::none()
}
