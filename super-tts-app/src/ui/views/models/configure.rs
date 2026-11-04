// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, settings, text};

use crate::core::app::AppModel;
use crate::daemon::backends::{BackendInfo, BackendOption, BackendSecret};
use crate::ui::messages::{BackendMessage, Message};

use super::surface::muted_text_color;

/// Body of the per-backend configuration sheet (shown in the right-side
/// context drawer): the backend's secrets (system keyring) and options (daemon
/// config) as one settings section. The drawer supplies the title and close
/// affordance, so there's no in-body header or Back button.
pub fn configure_sheet<'a>(backend: &'a BackendInfo, app: &'a AppModel) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();

    let mut body: Vec<Element<'a, Message>> = Vec::new();

    // Surface a failed secret/option save inline in the sheet (Tier 1 #15)
    // instead of dropping it to the log.
    if let Some(message) = app.action_error_for(crate::state::ErrorScope::ConfigureBackend) {
        body.push(crate::ui::views::common::error_banner(message));
    }

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
            // An option that names the values it takes has nothing to type, so
            // nothing to type wrong: it gets a dropdown of exactly those values
            // instead of a box the daemon would have to reject. It is what an
            // option whose allowed values only ever lived in its help text
            // always wanted to be.
            if option.has_choices() {
                section = section.add(choice_option_row(&backend.source, option));
                continue;
            }
            // A numeric option bounded at both ends and moving in a declared
            // increment has nothing left to type: both ends and the grid are
            // the backend's, so the control is a slider and the only thing the
            // user supplies is where on it to stop.
            if option.is_slider() {
                let key = (backend.source.clone(), option.name.clone());
                let pending = app.backend_option_inputs.get(&key).map(String::as_str);
                section = section.add(slider_option_row(&backend.source, option, pending));
                continue;
            }
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
/// When `required` is true, an accent-colored asterisk follows the title to
/// signal that the field must be filled.
pub(super) fn config_label<'a>(
    title: String,
    hint: Option<String>,
    required: bool,
) -> Element<'a, Message> {
    let mut block = widget::column::with_capacity(2).spacing(cosmic::theme::spacing().space_xxxs);
    let title_row: Element<'a, Message> = if required {
        row![
            text::body(title),
            text::body(" *").class(cosmic::theme::Text::Accent),
        ]
        .into()
    } else {
        text::body(title).into()
    };
    block = block.push(title_row);
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
    let label = config_label(display, description, secret.required);

    let source_owned = source.to_string();
    let name_owned = secret.name.clone();

    let control: Element<'a, Message> = if configured {
        let remove_source = source_owned.clone();
        let remove_name = name_owned.clone();
        row![
            text::body("Configured").width(Length::Fill),
            widget::button::destructive("Remove").on_press(Message::Backend(
                BackendMessage::BackendSecretRemoved {
                    source: remove_source,
                    name: remove_name,
                },
            )),
        ]
        .spacing(spacing.space_s)
        .align_y(Alignment::Center)
        .into()
    } else {
        let input_source = source_owned.clone();
        let input_name = name_owned.clone();
        let field = widget::text_input("Enter API key...", input)
            .on_input(move |value| {
                Message::Backend(BackendMessage::BackendSecretInputChanged {
                    source: input_source.clone(),
                    name: input_name.clone(),
                    value,
                })
            })
            .password()
            .width(Length::Fill);
        let save = widget::button::standard("Save").on_press(Message::Backend(
            BackendMessage::BackendSecretSaved {
                source: source_owned,
                name: name_owned,
            },
        ));
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

/// The caption under an option's label: its description, with the backend's
/// declared default named alongside it.
///
/// Naming the default is what lets the row be read at all. The daemon reports
/// only the *effective* value, so a row showing `formal` says nothing about
/// whether that came from the backend or from a previous override the user has
/// forgotten setting — and in the dropdown's case, which entry to pick to hand
/// the option back to the backend. Shared by the text field and the dropdown so
/// the two cannot drift into describing the same option differently.
fn option_hint(option: &BackendOption) -> String {
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
    hint
}

/// One option row for an option that declares `choices`: the label/description
/// over a full-width dropdown of the values the backend accepts, laid out like
/// the text-field row so the sheet's rows read as one column.
///
/// There is no Save button and no Reset button — picking is the write, and
/// picking the declared default is itself the reset, since the handler turns
/// that pick into a clear rather than storing a copy of the default.
///
/// A stored value the backend no longer offers leaves the dropdown on its
/// placeholder rather than selecting a wrong row: [`BackendOption::choice_index`]
/// returns `None` for a value that is not on the list, so the user is shown
/// that nothing valid is selected and picks again, instead of being quietly
/// told they had chosen something they never did.
pub(super) fn choice_option_row<'a>(
    source: &'a str,
    option: &'a BackendOption,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let display = option.label.clone().unwrap_or_else(|| option.name.clone());
    let hint = option_hint(option);
    let label = config_label(display, (!hint.is_empty()).then_some(hint), option.required);

    // The message carries the chosen value, not its index: the catalog can be
    // replaced by a reload between render and press, and an index into a list
    // that has since changed would write the wrong choice.
    let pick_source = source.to_string();
    let pick_name = option.name.clone();
    let pick_choices = option.choices.clone();
    let dropdown = widget::dropdown(
        option.choices.as_slice(),
        option.choice_index(),
        move |index| {
            Message::Backend(BackendMessage::BackendOptionChosen {
                source: pick_source.clone(),
                name: pick_name.clone(),
                value: pick_choices[index].clone(),
            })
        },
    )
    .placeholder("Select")
    .width(Length::Fill);

    settings::item_row(vec![
        column![label, dropdown]
            .spacing(spacing.space_xs)
            .width(Length::Fill)
            .into(),
    ])
    .into()
}

/// How many decimals it takes to write `step` without losing it.
///
/// A slider over a fractional step has to commit the value the user stopped
/// on, not the binary double nearest it: a 0.1 step from 0.6 reaches
/// 0.7000000000000001, and storing that would show it back in the settings
/// sheet and inject it into a header. Six is where a settings control stops
/// being one.
fn step_decimals(step: f64) -> usize {
    (0..=6)
        .find(|&d| format!("{step:.d$}").parse::<f64>() == Ok(step))
        .unwrap_or(6)
}

/// One option row for a numeric option that declares `min`, `max` and `step`:
/// the label/description over a slider, with the value it is resting on shown
/// beside it.
///
/// Dragging updates the pending value only; the write fires once on release,
/// the way the volume slider does, so a single drag is not a hundred `POST`s.
/// The pending value rides in the same map the text rows use, so this borrows
/// their save path whole rather than growing a second one.
///
/// There is no Save button for the same reason the dropdown has none: letting
/// go is the write.
pub(super) fn slider_option_row<'a>(
    source: &'a str,
    option: &'a BackendOption,
    pending: Option<&'a str>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let display = option.label.clone().unwrap_or_else(|| option.name.clone());
    let hint = option_hint(option);
    let label = config_label(display, (!hint.is_empty()).then_some(hint), option.required);

    // `is_slider` is what routed us here, so all three are present.
    let (low, high, step) = (
        option.min.unwrap_or(0.0),
        option.max.unwrap_or(1.0),
        option.step.unwrap_or(1.0),
    );
    let decimals = step_decimals(step);
    // What the slider rests on: the value being dragged, else what is in
    // effect, else the low end — an option with no value and no default has
    // to put the handle somewhere, and the bottom of its own range is the
    // only choice that is certainly in it.
    let parse = |v: &str| v.trim().parse::<f64>().ok();
    let current = pending
        .and_then(parse)
        .or_else(|| option.value.as_deref().and_then(parse))
        .unwrap_or(low)
        .clamp(low, high);

    let drag_source = source.to_string();
    let drag_name = option.name.clone();
    let slider = widget::slider(low..=high, current, move |value| {
        Message::Backend(BackendMessage::BackendOptionInputChanged {
            source: drag_source.clone(),
            name: drag_name.clone(),
            value: format!("{value:.decimals$}"),
        })
    })
    .step(step)
    .on_release(Message::Backend(BackendMessage::BackendOptionSaved {
        source: source.to_string(),
        name: option.name.clone(),
    }))
    .width(Length::Fill);

    let reading = text::body(format!("{current:.decimals$}"))
        .width(Length::Fixed(44.0))
        .align_x(Alignment::End);

    settings::item_row(vec![
        column![
            label,
            row![slider, reading]
                .spacing(spacing.space_xs)
                .align_y(Alignment::Center),
        ]
        .spacing(spacing.space_xs)
        .width(Length::Fill)
        .into(),
    ])
    .into()
}

/// One option-entry row for a backend (e.g. `base_url`): the label/description
/// over a full-width text field + Save on its own row beneath, so the input
/// isn't squeezed to the right of the label in the narrow sheet.
/// When an override is active (stored value differs from the default), a Reset
/// button is shown alongside Save to clear the override and revert to the
/// daemon default.
pub(super) fn option_row<'a>(
    source: &'a str,
    option: &'a BackendOption,
    input: &'a str,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let display = option.label.clone().unwrap_or_else(|| option.name.clone());
    let hint = option_hint(option);

    let label = config_label(display, (!hint.is_empty()).then_some(hint), option.required);

    let input_source = source.to_string();
    let input_name = option.name.clone();
    let save_source = input_source.clone();
    let save_name = input_name.clone();

    let field = widget::text_input("", input)
        .on_input(move |value| {
            Message::Backend(BackendMessage::BackendOptionInputChanged {
                source: input_source.clone(),
                name: input_name.clone(),
                value,
            })
        })
        .width(Length::Fill);
    let save = widget::button::standard("Save").on_press(Message::Backend(
        BackendMessage::BackendOptionSaved {
            source: save_source,
            name: save_name,
        },
    ));

    // Show Reset only when an override is stored (value differs from default).
    let has_override = option.value.as_deref() != option.default.as_deref();
    let control: Element<'a, Message> = if has_override {
        let reset_source = source.to_string();
        let reset_name = option.name.clone();
        let reset = widget::button::standard("Reset").on_press(Message::Backend(
            BackendMessage::BackendOptionReset {
                source: reset_source,
                name: reset_name,
            },
        ));
        row![field, save, reset]
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center)
            .into()
    } else {
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

#[cfg(test)]
mod tests {
    use super::step_decimals;

    /// A slider commits what the user stopped on, and a fractional step does
    /// not land on round doubles: 0.6 + 0.1 is 0.7000000000000001. Writing the
    /// value back at the step's own precision is what keeps that out of the
    /// settings sheet and out of the header the daemon injects.
    #[test]
    fn a_step_is_written_at_its_own_precision() {
        assert_eq!(step_decimals(1.0), 0);
        assert_eq!(step_decimals(5.0), 0);
        assert_eq!(step_decimals(0.5), 1);
        assert_eq!(step_decimals(0.1), 1);
        assert_eq!(step_decimals(0.25), 2);
        assert_eq!(step_decimals(0.001), 3);
    }

    /// The rounded value has to read back as a number inside the same range,
    /// or the slider would write something the daemon then refuses.
    #[test]
    fn a_rounded_step_still_parses() {
        let (low, high, step) = (0.6_f64, 1.2_f64, 0.1_f64);
        let decimals = step_decimals(step);
        let mut value = low;
        while value <= high + f64::EPSILON {
            let written = format!("{value:.decimals$}");
            let parsed: f64 = written.parse().expect("a slider writes a number");
            assert!(
                (low..=high).contains(&parsed),
                "{written} fell outside {low}..={high}"
            );
            value += step;
        }
    }

    /// A step no decimal count can express falls back rather than looping or
    /// writing a value that does not round-trip.
    #[test]
    fn an_unrepresentable_step_falls_back() {
        assert_eq!(step_decimals(1.0 / 3.0), 6);
    }
}
