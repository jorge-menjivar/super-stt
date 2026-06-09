// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::iced_widget::{column, row};
use cosmic::widget::{self, settings, text};

use crate::core::app::AppModel;
use crate::daemon::backends::{BackendInfo, BackendOption, BackendSecret};
use crate::ui::messages::Message;

use super::surface::muted_text_color;

/// Body of the per-backend configuration sheet (shown in the right-side
/// context drawer): the backend's secrets (system keyring) and options (daemon
/// config) as one settings section. The drawer supplies the title and close
/// affordance, so there's no in-body header or Back button.
pub fn configure_sheet<'a>(backend: &'a BackendInfo, app: &'a AppModel) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();

    let mut body: Vec<Element<'a, Message>> = Vec::new();

    if backend.secrets.is_empty() && backend.options.is_empty() {
        body.push(text::body("This backend has nothing to configure.").into());
    } else {
        let mut section = settings::section();
        for secret in &backend.secrets {
            let key = (backend.source.clone(), secret.name.clone());
            let configured = app
                .backend_secret_configured
                .get(&key)
                .copied()
                .unwrap_or(false);
            let input = app
                .backend_secret_inputs
                .get(&key)
                .map_or("", String::as_str);
            section = section.add(secret_row(&backend.source, secret, configured, input));
        }
        for option in &backend.options {
            let key = (backend.source.clone(), option.name.clone());
            let input = app
                .backend_option_inputs
                .get(&key)
                .map_or("", String::as_str);
            section = section.add(option_row(&backend.source, option, input));
        }
        body.push(section.into());
    }

    column(body)
        .spacing(spacing.space_m)
        .width(Length::Fill)
        .into()
}

/// A label + optional caption stacked vertically, used as the heading above a
/// configuration row's control. The caption is dimmed so it reads as a hint.
pub(super) fn config_label<'a>(title: String, hint: Option<String>) -> Element<'a, Message> {
    let mut block = widget::column::with_capacity(2).spacing(cosmic::theme::spacing().space_xxxs);
    block = block.push(text::body(title));
    if let Some(hint) = hint.filter(|h| !h.is_empty()) {
        block =
            block.push(text::caption(hint).class(cosmic::theme::Text::Color(muted_text_color())));
    }
    block.into()
}

/// One secret-entry row for a backend (e.g. an API key): the label/description
/// over a full-width password field + Save when unconfigured, or a "Configured"
/// badge + Remove when stored. The control gets its own row beneath the label
/// so the input spans the (narrow) sheet width instead of being squeezed to the
/// right of the label.
pub(super) fn secret_row<'a>(
    source: &'a str,
    secret: &'a BackendSecret,
    configured: bool,
    input: &'a str,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    // Show the backend's human label when set; fall back to the technical name.
    let display = secret.label.clone().unwrap_or_else(|| secret.name.clone());
    let description = (!secret.description.is_empty()).then(|| secret.description.clone());
    let label = config_label(display, description);

    let source_owned = source.to_string();
    let name_owned = secret.name.clone();

    let control: Element<'a, Message> = if configured {
        let remove_source = source_owned.clone();
        let remove_name = name_owned.clone();
        row![
            text::body("Configured").width(Length::Fill),
            widget::button::destructive("Remove").on_press(Message::BackendSecretRemoved {
                source: remove_source,
                name: remove_name,
            }),
        ]
        .spacing(spacing.space_s)
        .align_y(Alignment::Center)
        .into()
    } else {
        let input_source = source_owned.clone();
        let input_name = name_owned.clone();
        let field = widget::text_input("Enter API key...", input)
            .on_input(move |value| Message::BackendSecretInputChanged {
                source: input_source.clone(),
                name: input_name.clone(),
                value,
            })
            .password()
            .width(Length::Fill);
        let save = widget::button::standard("Save").on_press(Message::BackendSecretSaved {
            source: source_owned,
            name: name_owned,
        });
        row![field, save]
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center)
            .into()
    };

    settings::item_row(vec![
        column![label, control]
            .spacing(spacing.space_xs)
            .width(Length::Fill)
            .into(),
    ])
    .into()
}

/// One option-entry row for a backend (e.g. `base_url`): the label/description
/// over a full-width text field + Save on its own row beneath, so the input
/// isn't squeezed to the right of the label in the narrow sheet.
pub(super) fn option_row<'a>(
    source: &'a str,
    option: &'a BackendOption,
    input: &'a str,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let display = option.label.clone().unwrap_or_else(|| option.name.clone());
    let mut hint = option.description.clone();
    if let Some(default) = &option.default
        && !default.is_empty()
    {
        if hint.is_empty() {
            hint = format!("Default: {default}");
        } else {
            hint = format!("{hint} (default: {default})");
        }
    }

    let label = config_label(display, (!hint.is_empty()).then_some(hint));

    let input_source = source.to_string();
    let input_name = option.name.clone();
    let save_source = input_source.clone();
    let save_name = input_name.clone();

    let field = widget::text_input("", input)
        .on_input(move |value| Message::BackendOptionInputChanged {
            source: input_source.clone(),
            name: input_name.clone(),
            value,
        })
        .width(Length::Fill);
    let save = widget::button::standard("Save").on_press(Message::BackendOptionSaved {
        source: save_source,
        name: save_name,
    });
    let control = row![field, save]
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center);

    settings::item_row(vec![
        column![label, control]
            .spacing(spacing.space_xs)
            .width(Length::Fill)
            .into(),
    ])
    .into()
}
