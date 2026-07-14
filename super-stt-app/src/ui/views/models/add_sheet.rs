// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::{Alignment, Length};
use cosmic::iced_widget::{column, row};
use cosmic::widget::{self, button, text};
use cosmic::{Apply, Element};

use crate::core::app::AppModel;
use crate::ui::icons;
use crate::ui::messages::{Message, ModelsPageMessage};

use super::surface::muted_text_color;

/// Right-side "Add a backend" sheet, rendered as a COSMIC context drawer. It
/// holds the two manual install paths that used to crowd the top of the
/// Download tab — install from a Git repository URL, or import a local
/// directory. The drawer is scoped to the Models page and dismisses itself on
/// navigation or daemon disconnect (enforced in `AppModel::context_drawer`).
pub fn add_backend_sheet(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let muted = muted_text_color();

    // From a repository: URL input + Install, with the unverified-source note.
    let can_install = !app.registry.custom_repo_input.trim().is_empty();
    let install_btn = if can_install {
        button::suggested("Install").on_press(Message::ModelsPage(
            ModelsPageMessage::InstallBackendFromRepoUrl(app.registry.custom_repo_input.clone()),
        ))
    } else {
        button::suggested("Install")
    };
    let repo_section = column![
        text::title4("From a repository"),
        text::body(
            "Paste a Git repository URL. Super STT resolves the latest release, \
             verifies its manifest, and installs it."
        )
        .class(cosmic::theme::Text::Color(muted)),
        row![
            widget::text_input(
                "https://github.com/owner/backend",
                &app.registry.custom_repo_input
            )
            .on_input(|x| Message::ModelsPage(ModelsPageMessage::RegistryCustomRepoInputChanged(x)))
            .width(Length::Fill),
            install_btn,
        ]
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center),
        row![
            icons::phosphor_warning(icons::WARNING, 15.0),
            text::caption("Unverified source — only HTTPS protects this download."),
        ]
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center),
    ]
    .spacing(spacing.space_s)
    .apply(widget::container)
    .class(cosmic::theme::Container::List)
    .padding(spacing.space_m)
    .width(Length::Fill);

    // From a folder: import a local backend directory.
    let dir_section = column![
        text::title4("From a folder"),
        text::body("Point Super STT at a local directory that contains a backend.toml manifest.")
            .class(cosmic::theme::Text::Color(muted)),
        button::standard("Choose folder\u{2026}")
            .on_press(Message::ModelsPage(ModelsPageMessage::ImportBackendFromDir)),
    ]
    .spacing(spacing.space_s)
    .apply(widget::container)
    .class(cosmic::theme::Container::List)
    .padding(spacing.space_m)
    .width(Length::Fill);

    column![repo_section, dir_section]
        .spacing(spacing.space_m)
        .width(Length::Fill)
        .into()
}
