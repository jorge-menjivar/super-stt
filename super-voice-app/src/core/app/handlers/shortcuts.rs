// SPDX-License-Identifier: GPL-3.0-only
//! The Shortcuts page: asking the daemon where each shortcut stands, and
//! adding or removing one.

use crate::core::app::AppModel;
use crate::daemon::client::v1::shortcuts::{add_shortcut, list_shortcuts, remove_shortcut};
use crate::state::ErrorScope;
use crate::ui::messages::{Message, ShortcutsMessage};
use cosmic::prelude::*;

/// Whether a change failed because the user turned the shortcut down in the
/// desktop's own dialog. That is their answer, not a failure, so the page
/// shows no error for it: it offers to add the shortcut again.
fn turned_down(error: &str) -> bool {
    error.contains("shortcut_refused")
}

impl AppModel {
    /// Route the Shortcuts page's messages. The daemon answers every change
    /// with the whole list, so the page shows what the desktop now has.
    pub(in crate::core::app) fn handle_shortcuts_messages(
        &mut self,
        message: ShortcutsMessage,
    ) -> Task<cosmic::Action<Message>> {
        let answer = |result: Result<_, super_voice_shared::daemon::http_client::HttpError>| {
            cosmic::Action::App(Message::Shortcuts(ShortcutsMessage::Changed(
                result.map_err(|e| e.to_string()),
            )))
        };
        match message {
            ShortcutsMessage::Refresh => Task::perform(list_shortcuts(), |result| {
                cosmic::Action::App(Message::Shortcuts(ShortcutsMessage::Loaded(
                    result.map_err(|e| e.to_string()),
                )))
            }),
            ShortcutsMessage::Add(id) => {
                self.clear_action_error(ErrorScope::Shortcuts);
                Task::perform(add_shortcut(id), answer)
            }
            ShortcutsMessage::Remove(id) => {
                self.clear_action_error(ErrorScope::Shortcuts);
                Task::perform(remove_shortcut(id), answer)
            }
            ShortcutsMessage::Loaded(Ok(list)) | ShortcutsMessage::Changed(Ok(list)) => {
                self.shortcuts.list = Some(list);
                Task::none()
            }
            ShortcutsMessage::Loaded(Err(error)) => {
                log::warn!("Could not read the shortcuts: {error}");
                self.set_action_error(
                    ErrorScope::Shortcuts,
                    crate::fl!("shortcuts-load-failed", error = error),
                );
                Task::none()
            }
            // Read the list again: what failed may have changed something.
            ShortcutsMessage::Changed(Err(error)) => {
                if turned_down(&error) {
                    log::info!("The shortcut was turned down in the desktop's dialog");
                } else {
                    log::warn!("Could not change a shortcut: {error}");
                    self.set_action_error(
                        ErrorScope::Shortcuts,
                        crate::fl!("shortcut-change-failed", error = error),
                    );
                }
                Task::done(cosmic::Action::App(Message::Shortcuts(
                    ShortcutsMessage::Refresh,
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::turned_down;

    /// Only the user saying no in the desktop's dialog goes without an error:
    /// every other failure is shown.
    #[test]
    fn only_a_refusal_is_the_user_turning_it_down() {
        assert!(turned_down("shortcut_refused (HTTP 409)"));
        assert!(!turned_down("shortcut_keys_taken (HTTP 409)"));
        assert!(!turned_down("shortcuts_unsupported (HTTP 409)"));
        assert!(!turned_down(
            "internal: could_not_write_shortcut: no session bus (HTTP 500)"
        ));
    }
}
