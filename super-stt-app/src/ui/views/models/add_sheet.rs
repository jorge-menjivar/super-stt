// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, space::horizontal as horizontal_space, text};
use cosmic::{Apply, Element};

use crate::core::app::AppModel;
use crate::state::registry::{AddSource, PreviewState};
use crate::ui::icons;
use crate::ui::messages::{Message, ModelsPageMessage};

use super::chips::{CloudEgress, capability_chips, count_chip, model_device_chips, role_label};
use super::surface::{card_divider, muted_text_color};

/// Right-side "Install manually" sheet, rendered as a COSMIC context drawer.
///
/// One path, not two. The sheet used to offer a repository card and a folder
/// card side by side, each with its own install button that fired straight at
/// the daemon: paste a URL, press Install, and either a backend appeared or a
/// line landed in a log. Here the source is a choice inside a single flow, and
/// resolving it is a separate step from committing to it — Check fills the
/// preview, Install commits what is in it.
///
/// The preview is where every outcome lands, successes and failures alike. That
/// is the point of it: a rejected resolve is keyed by the pasted URL, which
/// matches no Browse card, so before this the only place it appeared was the
/// log (issue #423).
pub fn add_backend_sheet(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let muted = muted_text_color();
    let source = app.registry.add_source;

    // Source row: label left, dropdown right, the COSMIC settings idiom.
    let selected = AddSource::ALL.iter().position(|s| *s == source);
    let source_row = row![
        text::body("Source").class(cosmic::theme::Text::Color(muted)),
        horizontal_space(),
        widget::dropdown(&SOURCE_LABELS, selected, |i| {
            Message::ModelsPage(ModelsPageMessage::AddSourceChanged(
                AddSource::ALL[i.min(AddSource::ALL.len() - 1)],
            ))
        }),
    ]
    .spacing(spacing.space_xs)
    .align_y(Alignment::Center);

    let entry_row = match source {
        AddSource::Repository => repository_entry(app, spacing.space_xs),
        AddSource::Folder => folder_entry(app, spacing.space_xs),
    };

    let sheet = column![
        source_row,
        entry_row,
        card_divider(),
        preview_panel(app, muted),
    ]
    .spacing(spacing.space_s)
    .apply(widget::container)
    .class(cosmic::theme::Container::List)
    .padding(spacing.space_m)
    .width(Length::Fill);

    sheet.into()
}

/// The drawer's pinned footer: the Install action.
///
/// Rendered through `ContextDrawer::footer` rather than as the last row of the
/// sheet, because the sheet's body scrolls. A resolved preview is tall enough
/// to push a button at the end of that column past the bottom of the drawer,
/// which left the one control the whole flow builds towards only reachable by
/// scrolling to it.
pub fn add_backend_footer(app: &AppModel) -> Element<'_, Message> {
    install_row(app, muted_text_color())
}

/// Dropdown labels, in [`AddSource::ALL`] order. `widget::dropdown` borrows
/// this for the life of the view, so it is a const rather than a temporary.
const SOURCE_LABELS: [&str; 2] = ["Git repository", "Local folder"];

/// URL field plus Check. Resolving a URL spends one of GitHub's 60
/// unauthenticated API calls an hour (`GITHUB_TOKEN` is optional, so most
/// installs have that budget), so the trip is worth an explicit press rather
/// than a debounce that would spend several per URL typed.
fn repository_entry(app: &AppModel, gap: u16) -> Element<'_, Message> {
    let url = app.registry.custom_repo_input.trim();
    let checking = app.registry.add_preview.is_checking();

    let mut check = button::standard(if checking {
        "Checking\u{2026}"
    } else {
        "Check"
    });
    if !checking && !url.is_empty() {
        check = check.on_press(Message::ModelsPage(ModelsPageMessage::CheckAddSource));
    }

    row![
        widget::text_input(
            "https://github.com/owner/backend",
            &app.registry.custom_repo_input
        )
        .on_input(|x| Message::ModelsPage(ModelsPageMessage::RegistryCustomRepoInputChanged(x)))
        .on_submit(|_| Message::ModelsPage(ModelsPageMessage::CheckAddSource))
        .width(Length::Fill),
        check,
    ]
    .spacing(gap)
    .align_y(Alignment::Center)
    .into()
}

/// Path field, the picker, and Check.
///
/// The path is typed as well as picked: a folder someone is developing a
/// backend in is a path they already know, and making them walk a file dialog
/// to a directory they could have pasted is the slower half of the flow. The
/// picker stays for the times the path is easier found than remembered.
///
/// Check is here for the same reason it is in the URL row — the field can hold
/// a path nothing has read yet — even though a local manifest costs no API
/// budget to read.
fn folder_entry(app: &AppModel, gap: u16) -> Element<'_, Message> {
    let path = app.registry.add_folder.trim();
    let checking = app.registry.add_preview.is_checking();

    let mut check = button::standard(if checking {
        "Checking\u{2026}"
    } else {
        "Check"
    });
    if !checking && !path.is_empty() {
        check = check.on_press(Message::ModelsPage(ModelsPageMessage::CheckAddSource));
    }

    row![
        widget::text_input("/home/you/dev/my-backend", &app.registry.add_folder)
            .on_input(|x| Message::ModelsPage(ModelsPageMessage::AddFolderInputChanged(x)))
            .on_submit(|_| Message::ModelsPage(ModelsPageMessage::CheckAddSource))
            .width(Length::Fill),
        button::standard("Choose\u{2026}")
            .on_press(Message::ModelsPage(ModelsPageMessage::ImportBackendFromDir)),
        check,
    ]
    .spacing(gap)
    .align_y(Alignment::Center)
    .into()
}

/// The preview: what this source would install, or why it could not be read.
fn preview_panel(app: &AppModel, muted: cosmic::iced::Color) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();

    let body: Element<'_, Message> = match &app.registry.add_preview {
        PreviewState::Empty => {
            let hint = match app.registry.add_source {
                AddSource::Repository => {
                    "Paste a repository URL and press Check. Super STT reads its latest release \
                     and shows what it found before anything is installed."
                }
                AddSource::Folder => {
                    "Choose a folder that holds a backend.toml manifest. Super STT reads it and \
                     shows what it found before anything is copied."
                }
            };
            column![
                text::body("Nothing to preview yet"),
                text::caption(hint).class(cosmic::theme::Text::Color(muted)),
            ]
            .spacing(spacing.space_xxs)
            .into()
        }

        // A spinner beside the line, not just the line: reading a release takes
        // a round-trip to the forge, and a static sentence gives no sign that
        // anything is still happening. The glyph is a still SVG, so the angle
        // comes from state and the app ticks it while the check is in flight.
        PreviewState::Checking(what) => row![
            icons::spinner(18.0, muted, app.registry.add_spin),
            text::body(format!("Reading {what}\u{2026}")).class(cosmic::theme::Text::Color(muted)),
        ]
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center)
        .into(),

        PreviewState::Failed(err) => row![
            icons::phosphor_destructive(icons::WARNING, 15.0),
            text::body(err.clone()).width(Length::Fill),
        ]
        .spacing(spacing.space_xs)
        .into(),

        PreviewState::Ready(backend) => resolved_preview(backend, muted),
    };

    widget::container(body)
        .padding(spacing.space_s)
        .width(Length::Fill)
        .class(cosmic::theme::Container::Primary)
        .into()
}

/// The resolved backend: identity, the facts a person is agreeing to, and the
/// models it serves.
fn resolved_preview(
    b: &super_stt_shared::registry::RegistryBackend,
    muted: cosmic::iced::Color,
) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let mut col = widget::column::with_capacity(9).spacing(spacing.space_xs);

    // An incompatible host is the most useful thing the panel can say, so it
    // says it first, above the name.
    if !b.compatibility.compatible {
        let reason = b
            .compatibility
            .reason
            .as_deref()
            .unwrap_or("This backend cannot run on this computer.");
        col = col.push(
            row![
                icons::phosphor_destructive(icons::WARNING, 15.0),
                text::body(reason.to_string()).width(Length::Fill),
            ]
            .spacing(spacing.space_xs),
        );
    }

    col = col.push(
        row![
            text::title4(b.name.clone()),
            text::body(b.version.clone()).class(cosmic::theme::Text::Color(muted)),
        ]
        .spacing(spacing.space_xs)
        .align_y(Alignment::End),
    );
    col = col.push(text::caption(b.source.clone()).class(cosmic::theme::Text::Color(muted)));

    if let Some(desc) = b.description.as_deref().filter(|d| !d.is_empty()) {
        col = col.push(text::body(desc.to_string()).class(cosmic::theme::Text::Color(muted)));
    }

    let egress = b.online.then_some(CloudEgress {
        hosts: b.allowed_hosts.as_slice(),
        user_url: false,
    });
    if let Some(chips) = capability_chips(b.supports_gpu, b.supports_cpu, egress, false) {
        col = col.push(chips);
    }

    col = col.push(card_divider());
    for (label, value, style) in facts(b) {
        let value = match style {
            FactStyle::Plain => text::caption(value),
            // A hostname is machine text, and setting it as such is what tells
            // the reader it is a literal address rather than prose.
            FactStyle::Mono => text::monotext(value),
            // "Nothing. Runs offline." is the one fact here that is good news,
            // and the person installing a microphone reader is looking for it.
            FactStyle::Good => text::caption(value).class(cosmic::theme::Text::Color(
                cosmic::theme::active().cosmic().success.base.into(),
            )),
        };
        col = col.push(
            row![
                text::caption(label)
                    .class(cosmic::theme::Text::Color(muted))
                    .width(Length::Fixed(86.0)),
                value.width(Length::Fill),
            ]
            .spacing(spacing.space_xs),
        );
    }

    // One row per model rather than one joined caption line: a preview is read
    // to find out what a backend actually serves, and names run together with
    // separators are exactly the shape that stops being read.
    if !b.models.is_empty() {
        col = col.push(card_divider());
        col = col.push(
            text::caption(if b.models.len() == 1 {
                "Model"
            } else {
                "Models"
            })
            .class(cosmic::theme::Text::Color(muted)),
        );
        for m in &b.models {
            let mut r = row![
                text::caption(m.name.clone()).width(Length::Fill),
                count_chip(role_label(&m.role).to_string()),
            ]
            .spacing(spacing.space_xxs)
            .align_y(Alignment::Center);
            for chip in model_device_chips(&m.supported_devices) {
                r = r.push(chip);
            }
            col = col.push(r);
        }
    }

    col.into()
}

/// How a fact's value should read.
#[derive(Clone, Copy)]
enum FactStyle {
    Plain,
    /// A literal machine value: a hostname, a target triple.
    Mono,
    /// Good news worth colouring.
    Good,
}

/// The label/value rows: what a person is actually agreeing to run.
fn facts(b: &super_stt_shared::registry::RegistryBackend) -> Vec<(String, String, FactStyle)> {
    let mut out = Vec::with_capacity(5);

    out.push((
        "Runs as".to_string(),
        match b.kind.as_str() {
            "wasm" => "WASM component, sandboxed".to_string(),
            "subprocess" => "Native subprocess".to_string(),
            other => other.to_string(),
        },
        FactStyle::Plain,
    ));

    // Only when there is something to get. A required secret is a real cost
    // the preview can warn about — installing is not enough, you have to go
    // fetch a credential before it will run — but every local backend needs
    // none, so a row that mostly reads "Nothing" is padding.
    let needed: Vec<&str> = b
        .secrets
        .iter()
        .filter(|s| s.required)
        .map(|s| s.label.as_str())
        .collect();
    if !needed.is_empty() {
        out.push(("Needs".to_string(), needed.join(", "), FactStyle::Plain));
    }

    // Always, including when it is empty: "nothing, runs offline" is the
    // reassurance someone installing a microphone reader is looking for, so
    // the absence is worth as much as the list.
    if b.allowed_hosts.is_empty() {
        out.push((
            "Reaches".to_string(),
            "Nothing. Runs offline.".to_string(),
            FactStyle::Good,
        ));
    } else {
        out.push((
            "Reaches".to_string(),
            b.allowed_hosts.join(", "),
            FactStyle::Mono,
        ));
    }

    if let Some(asset) = &b.compatibility.selected_asset
        && !asset.target.is_empty()
    {
        let accel = asset.accel.join(", ");
        out.push((
            "Built for".to_string(),
            if accel.is_empty() {
                asset.target.clone()
            } else {
                format!("{}, {accel}", asset.target)
            },
            FactStyle::Mono,
        ));
    }

    out.push(("License".to_string(), b.license.clone(), FactStyle::Plain));
    out
}

/// Footer: what the button will do on the left, the button on the right.
///
/// Install commits the previewed backend, so it is live only while there is a
/// preview to commit and this machine can run it. Its label names what it
/// installs, so the press and its result read as the same act.
fn install_row(app: &AppModel, muted: cosmic::iced::Color) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let ready = app.registry.add_preview.ready();

    let key = app.registry.add_key();
    let in_flight = app.registry.installs.get(&key);
    let start_error = app.registry.install_errors.get(&key);

    let (label, live) = match (in_flight, ready) {
        (Some(s), _) => (
            match &s.error {
                Some(_) => "Failed".to_string(),
                None => format!(
                    "Installing\u{2026} ({})",
                    super::download::phase_label(s.phase)
                ),
            },
            false,
        ),
        (None, Some(b)) if b.compatibility.compatible => (format!("Install {}", b.name), true),
        _ => ("Install".to_string(), false),
    };

    let mut btn = button::suggested(label);
    if live {
        btn = btn.on_press(Message::ModelsPage(ModelsPageMessage::InstallPreviewed));
    }

    let note: Element<'_, Message> = if let Some(err) = start_error {
        row![
            icons::phosphor_destructive(icons::WARNING, 14.0),
            text::caption(format!("Failed to start: {err}")),
        ]
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center)
        .into()
    } else if let Some(s) = in_flight {
        match (&s.error, s.bytes_total) {
            (Some(err), _) => text::caption(format!("Failed: {err}"))
                .class(cosmic::theme::Text::Color(muted))
                .into(),
            (None, Some(total)) if total > 0 => {
                text::caption(format!("{}%", (s.bytes_done * 100) / total))
                    .class(cosmic::theme::Text::Color(muted))
                    .into()
            }
            _ => horizontal_space().into(),
        }
    } else if let Some(b) = ready {
        // What the press will install, beside the press. Without it the footer
        // is a bare button and the version it commits to is only readable by
        // scrolling the preview back up.
        let where_from = match app.registry.add_source {
            AddSource::Repository => "from its latest release",
            AddSource::Folder => "copied from disk",
        };
        text::caption(format!("Version {}, {where_from}", b.version))
            .class(cosmic::theme::Text::Color(muted))
            .into()
    } else {
        horizontal_space().into()
    };

    row![widget::container(note).width(Length::Fill), btn]
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}
