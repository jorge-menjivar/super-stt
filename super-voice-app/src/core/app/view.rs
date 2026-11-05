// SPDX-License-Identifier: GPL-3.0-only

use crate::state::{ContextPage, DaemonStatus, Page};
use crate::ui::messages::{
    ContextsMessage, LanguageMessage, Message, ModelsPageMessage, ShellMessage,
};
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

        let mut row = widget::row::with_capacity(3)
            .spacing(8.0)
            .align_y(cosmic::iced::Alignment::Center);
        if let Some(gpu) = views::models::gpu_summary(self) {
            row = row.push(gpu);
        }
        row = row.push(views::models::status_pill(self));
        if let Some(badge) = views::updates::header_badge(self) {
            row = row.push(badge);
        }

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
                    .title("Install manually")
                    // Outside the scrollable body: a resolved preview is tall
                    // enough to scroll the Install button off the bottom.
                    .footer(views::models::add_backend_footer(self))
                })
            }
            // The post-processing twin of the Select-a-backend sheet, scoped
            // the same way: it lists the backends that serve a post-processor
            // and selects the chosen one.
            ContextPage::SelectPostProcessor => {
                let on_models_page = self.daemon_status == DaemonStatus::Connected
                    && matches!(self.nav.data::<Page>(self.nav.active()), Some(Page::Models));
                on_models_page.then(|| {
                    context_drawer::context_drawer(
                        views::models::post_processor_sheet(self),
                        Message::Shell(ShellMessage::ToggleContextPage(
                            ContextPage::SelectPostProcessor,
                        )),
                    )
                    .title("Select post-processor backend")
                })
            }
            // The "Select a backend" sheet is scoped to the Models page: it
            // lists installed backends and activates the chosen one.
            ContextPage::SelectBackend => {
                let on_models_page = self.daemon_status == DaemonStatus::Connected
                    && matches!(self.nav.data::<Page>(self.nav.active()), Some(Page::Models));
                on_models_page.then(|| {
                    context_drawer::context_drawer(
                        views::models::select_backend_sheet(self),
                        Message::Shell(ShellMessage::ToggleContextPage(ContextPage::SelectBackend)),
                    )
                    .title("Select transcription backend")
                })
            }
            // Language picker sheet — a search-box + scrollable selectable list
            // for setting the global Primary Language or the active-model override.
            ContextPage::LanguagePicker => {
                let title = if self.language.language_picker_target.is_some() {
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
            // The context editor, scoped to the Contexts page and to a draft
            // actually being open.
            ContextPage::EditContext => self.context_editor_drawer(),
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
                    .models_page
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

    /// The context editor drawer, or nothing when it does not apply.
    ///
    /// Two conditions, and both are load-bearing. The page, because the sheet
    /// edits a row of a list that is only on the Contexts page — navigating
    /// away should dismiss it, and scoping it here is what does that without
    /// extra bookkeeping. The draft, because the draft *is* what the sheet
    /// edits: without one there is nothing to draw.
    fn context_editor_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Message>> {
        let on_contexts_page = self.daemon_status == DaemonStatus::Connected
            && matches!(
                self.nav.data::<Page>(self.nav.active()),
                Some(Page::Contexts)
            );
        if !on_contexts_page {
            return None;
        }
        let title = self
            .contexts
            .draft
            .as_ref()
            .map_or_else(|| "Edit context".to_string(), views::contexts::editor_title);
        let body = views::contexts::editor_sheet(self)?;
        Some(
            context_drawer::context_drawer(body, Message::Contexts(ContextsMessage::CancelEdit))
                .title(title),
        )
    }

    /// Describes the interface based on the current state of the application model.
    ///
    /// Application events will be processed through the view. Any messages emitted by
    /// events received by widgets will be passed to the update method.
    pub(super) fn view_impl(&self) -> Element<'_, Message> {
        #[cfg(target_os = "macos")]
        {
            cosmic::widget::column::with_capacity(2)
                .push(self.macos_toolbar())
                .push(self.page_view())
                .into()
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.page_view()
        }
    }

    /// The active page, or the connection page while disconnected.
    fn page_view(&self) -> Element<'_, Message> {
        // Force Connection page when daemon is not connected
        if self.daemon_status != DaemonStatus::Connected {
            return views::connection::page(self);
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
                self.language.primary_language.as_deref(),
                self.action_error_for(crate::state::ErrorScope::Customization),
            ),
            Page::Recording => views::recording::page(
                self.recording_stop_mode,
                self.preview_typing_enabled,
                self.notification_method,
                &self.recording_status,
                &self.transcription_text,
                &self.preview_text,
                self.preview_source,
                self.audio_level,
                self.is_speech_detected,
                self.action_error_for(crate::state::ErrorScope::Recording),
            ),
            Page::InputSimulation => views::input_simulation::page(
                self.write_method,
                &self.write_method_test_text,
                self.resolved_write_method,
                self.write_method_test_countdown,
                self.action_error_for(crate::state::ErrorScope::InputSimulation),
            ),
            Page::Models => views::models::page(self),
            Page::Library => views::models::library_page(self),
            Page::Contexts => views::contexts::page(self),
            Page::Updates => views::updates::page(&self.update),
            Page::Connection => views::connection::page(self),
        }
    }
}
