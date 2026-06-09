// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::ui::messages::Message;
use crate::ui::views;
use cosmic::prelude::*;

impl AppModel {
    /// Handle template/shell messages: URL opening, context page toggles, and URL launching.
    pub(in crate::core::app) fn handle_shell_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::OpenRepositoryUrl => {
                _ = open::that_detached(views::about::REPOSITORY);
                Task::none()
            }

            Message::ToggleContextPage(context_page) => {
                if self.context_page == context_page {
                    self.core.window.show_context = !self.core.window.show_context;
                } else {
                    self.context_page = context_page;
                    self.core.window.show_context = true;
                }
                Task::none()
            }

            Message::LaunchUrl(url) => {
                match open::that_detached(&url) {
                    Ok(()) => {}
                    Err(err) => {
                        eprintln!("failed to open {url:?}: {err}");
                    }
                }
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
