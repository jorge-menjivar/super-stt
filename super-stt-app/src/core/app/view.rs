// SPDX-License-Identifier: GPL-3.0-only

use crate::state::{ContextPage, DaemonStatus, Page};
use crate::ui::messages::{LanguageMessage, Message, ModelsPageMessage, ShellMessage};
use crate::ui::views;
use cosmic::app::context_drawer;
use cosmic::prelude::*;
use cosmic::widget::{self, nav_bar};

use super::AppModel;

impl AppModel {
    /// Window header-bar readouts (right side): the GPU summary and model
    /// readiness pills. Shown only while connected — when the daemon is down the
    /// app already renders a full-screen connection warning, so the title bar
    /// stays clean and needs no separate connection indicator. Both reuse the
    /// Models page's header-pill helpers, which are built from fixed pixels so
    /// they fit the title bar's fixed height without compressing (theme-spaced
    /// padding would overflow it at generous spacing and squish the dots into
    /// ovals).
    pub(super) fn header_end_impl(&self) -> Vec<Element<'_, Message>> {
        // No daemon → the body shows the connection warning; keep the title bar
        // empty rather than surfacing stale GPU / readiness readouts.
        if self.daemon_status != DaemonStatus::Connected {
            return Vec::new();
        }

        let mut row = widget::row::with_capacity(2)
            .spacing(8.0)
            .align_y(cosmic::iced::Alignment::Center);
        if let Some(gpu) = views::models::gpu_summary(self) {
            row = row.push(gpu);
        }
        row = row.push(views::models::status_pill(self));

        // Fixed trailing gap so the readouts aren't flush with the window edge.
        vec![widget::container(row).padding([0, 12, 0, 0]).into()]
    }

    /// Enables the COSMIC application to create a nav bar with this model.
    pub(super) fn nav_model_impl(&self) -> Option<&nav_bar::Model> {
        // Only show navigation when daemon is connected
        if self.daemon_status == DaemonStatus::Connected {
            Some(&self.nav)
        } else {
            None
        }
    }

    /// Display a context drawer if the context page is requested.
    pub(super) fn context_drawer_impl(&self) -> Option<context_drawer::ContextDrawer<'_, Message>> {
        if !self.core.window.show_context {
            return None;
        }

        match self.context_page {
            ContextPage::About => Some(
                context_drawer::context_drawer(
                    views::about::page(),
                    Message::Shell(ShellMessage::ToggleContextPage(ContextPage::About)),
                )
                .title("About"),
            ),
            // The Add-backend sheet is scoped to the Library page (its Browse
            // tab) while the daemon is connected; navigating away or a dropped
            // connection both dismiss it here, no extra bookkeeping required.
            ContextPage::AddBackend => {
                let on_library_page = self.daemon_status == DaemonStatus::Connected
                    && matches!(
                        self.nav.data::<Page>(self.nav.active()),
                        Some(Page::Library)
                    );
                on_library_page.then(|| {
                    context_drawer::context_drawer(
                        views::models::add_backend_sheet(self),
                        Message::Shell(ShellMessage::ToggleContextPage(ContextPage::AddBackend)),
                    )
                    .title("Add a backend")
                })
            }
            // The "Load a backend" sheet is scoped to the Models page: it lists
            // installed backends and activates the chosen one.
            ContextPage::LoadBackend => {
                let on_models_page = self.daemon_status == DaemonStatus::Connected
                    && matches!(self.nav.data::<Page>(self.nav.active()), Some(Page::Models));
                on_models_page.then(|| {
                    context_drawer::context_drawer(
                        views::models::load_backend_sheet(self),
                        Message::Shell(ShellMessage::ToggleContextPage(ContextPage::LoadBackend)),
                    )
                    .title("Load a backend")
                })
            }
            // Language picker sheet — a search-box + scrollable selectable list
            // for setting the global Primary Language or the active-model override.
            ContextPage::LanguagePicker => {
                let title = if self.language_picker_target.is_some() {
                    "Model language"
                } else {
                    "Primary Language"
                };
                Some(
                    context_drawer::context_drawer(
                        views::language_picker::sheet(self),
                        Message::Language(LanguageMessage::CloseLanguagePicker),
                    )
                    .title(title),
                )
            }
            // Per-backend configuration sheet — reachable from the active card
            // (Models) and from each installed card (Library), so it's scoped to
            // either page, and only when a backend is selected for configuration.
            ContextPage::ConfigureBackend => {
                let on_backend_page = self.daemon_status == DaemonStatus::Connected
                    && matches!(
                        self.nav.data::<Page>(self.nav.active()),
                        Some(Page::Models | Page::Library)
                    );
                let backend = self
                    .configure_backend
                    .as_ref()
                    .and_then(|src| self.backends.iter().find(|b| &b.source == src));
                backend.filter(|_| on_backend_page).map(|backend| {
                    context_drawer::context_drawer(
                        views::models::configure_sheet(backend, self),
                        Message::ModelsPage(ModelsPageMessage::CloseBackendConfig),
                    )
                    .title(format!("{} configuration", backend.name))
                })
            }
        }
    }

    /// Describes the interface based on the current state of the application model.
    ///
    /// Application events will be processed through the view. Any messages emitted by
    /// events received by widgets will be passed to the update method.
    pub(super) fn view_impl(&self) -> Element<'_, Message> {
        // Force Connection page when daemon is not connected
        if self.daemon_status != DaemonStatus::Connected {
            return views::connection::page(
                &self.daemon_status,
                self.socket_path.to_string_lossy().to_string(),
            );
        }

        // When connected, show normal navigation
        let active_page = self
            .nav
            .data::<Page>(self.nav.active())
            .unwrap_or(&Page::Customization);

        match active_page {
            Page::Customization => views::customization::page(
                &self.audio_themes,
                &self.selected_audio_theme,
                self.volume,
                self.primary_language.as_deref(),
                self.action_error_for(crate::state::ErrorScope::Customization),
            ),
            Page::Recording => views::recording::page(
                self.recording_stop_mode,
                self.preview_typing_enabled,
                &self.recording_status,
                &self.transcription_text,
                self.audio_level,
                self.is_speech_detected,
                self.action_error_for(crate::state::ErrorScope::Recording),
            ),
            Page::InputSimulation => views::input_simulation::page(
                self.write_method,
                self.action_error_for(crate::state::ErrorScope::InputSimulation),
            ),
            Page::Models => views::models::page(self),
            Page::Library => views::models::library_page(self),
            Page::Connection => views::connection::page(
                &self.daemon_status,
                self.socket_path.to_string_lossy().to_string(),
            ),
        }
    }
}
