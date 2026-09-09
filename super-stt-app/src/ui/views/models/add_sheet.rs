// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
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
    //
    // A custom-repo install is tracked under the string that was sent, which on
    // this path is exactly the text in the box — every other install is keyed
    // by a registry `source` and draws its state on a Browse card. So without
    // these two lookups the drawer showed nothing at all: a rejected install
    // reached only the log, which is where the reporter of issue #423 had to go
    // to find out why the button did nothing.
    let url = app.registry.custom_repo_input.as_str();
    let in_flight = app.registry.installs.get(url);
    let start_error = app.registry.install_errors.get(url);

    let install_btn: Element<'_, Message> = if let Some(s) = in_flight {
        let label = match &s.error {
            Some(_) => "Failed".to_string(),
            None => format!(
                "Installing\u{2026} ({})",
                super::download::phase_label(s.phase)
            ),
        };
        button::standard(label).into()
    } else if url.trim().is_empty() {
        button::suggested("Install").into()
    } else {
        button::suggested("Install")
            .on_press(Message::ModelsPage(
                ModelsPageMessage::InstallBackendFromRepoUrl(url.to_string()),
            ))
            .into()
    };

    // Same vocabulary as the Browse card: "Failed to start" for a request the
    // daemon rejected outright, "Failed" for one that died mid-install.
    let (failure, progress): (Option<String>, Option<String>) = if let Some(s) = in_flight {
        match (&s.error, s.bytes_total) {
            (Some(err), _) => (Some(format!("Failed: {err}")), None),
            (None, Some(total)) if total > 0 => {
                (None, Some(format!("{}%", (s.bytes_done * 100) / total)))
            }
            _ => (None, None),
        }
    } else {
        (
            start_error.map(|err| format!("Failed to start: {err}")),
            None,
        )
    };

    let mut repo_section = column![
        text::title4("From a repository"),
        text::body(
            "Paste a Git repository URL. Super STT resolves the latest release, \
             verifies its manifest, and installs it."
        )
        .class(cosmic::theme::Text::Color(muted)),
        row![
            widget::text_input("https://github.com/owner/backend", url)
                .on_input(|x| Message::ModelsPage(
                    ModelsPageMessage::RegistryCustomRepoInputChanged(x)
                ))
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
    ];

    if let Some(msg) = failure {
        repo_section = repo_section.push(
            row![
                icons::phosphor_destructive(icons::WARNING, 15.0),
                text::caption(msg),
            ]
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center),
        );
    } else if let Some(pct) = progress {
        repo_section =
            repo_section.push(text::caption(pct).class(cosmic::theme::Text::Color(muted)));
    }

    let repo_section = repo_section
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
