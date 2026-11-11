// SPDX-License-Identifier: GPL-3.0-only
//! The Contexts list: one card per context, one of them active.

use cosmic::Element;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, space::horizontal as horizontal_space, text};
use super_voice_shared::models::contexts::DictationContext;

use crate::core::app::AppModel;
use crate::core::app::scene::{self, Target};
use crate::ui::icons;
use crate::ui::messages::{ContextsMessage, Message};
use crate::ui::views::common;

use super::surface::{card, muted, plain_divider};

/// The page: a short explanation, the list, and a button to add one.
pub fn page(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let state = &app.contexts;

    let mut body = widget::column::with_capacity(5)
        .spacing(spacing.space_s)
        .width(Length::Fill);

    if let Some(message) = app.action_error_for(crate::state::ErrorScope::Contexts) {
        body = body.push(common::error_banner(message));
    }

    body = body.push(intro(app));

    if !state.loaded {
        body = body.push(text::body(crate::fl!("loading-progress")));
    } else if state.items.is_empty() {
        body = body.push(empty_state());
    } else {
        let cards: Vec<Element<'_, Message>> = state
            .items
            .iter()
            .map(|context| context_card(context, app))
            .collect();
        body = body.push(column(cards).spacing(spacing.space_s).width(Length::Fill));
        // Only worth offering once there is something to turn off.
        body = body.push(use_none_row(state.active.is_none()));
    }

    body = body.push(
        row![
            horizontal_space(),
            button::suggested(crate::fl!("new-context"))
                .on_press(Message::Contexts(ContextsMessage::Create)),
        ]
        .width(Length::Fill),
    );

    body.into()
}

/// What a context is for, in the fewest words that actually explain it.
///
/// Worth the space: nothing else on screen says that a vocabulary is what makes
/// a model hear "main branch" instead of "Maine branch", and a user who has not
/// been told will not guess it from a list of names.
fn intro(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let mut lines = widget::column::with_capacity(2).spacing(spacing.space_xxs);
    lines = lines.push(text::body(crate::fl!("a-context-tells-a-model-what-you")));

    // Which backends will actually use it. A context reaching nothing is the
    // one failure the user cannot see, so say it here rather than let them
    // wonder why nothing changed.
    let accepting = app.backends.iter().filter(|b| b.accepts_context).count();
    let note = if app.backends.is_empty() {
        None
    } else if accepting == 0 {
        Some(crate::fl!("none-of-your-installed-backends-can-use"))
    } else if accepting == app.backends.len() {
        None
    } else {
        Some(crate::fl!(
            "context-supported-count",
            accepting = (accepting).to_string(),
            value0 = (app.backends.len()).to_string()
        ))
    };
    if let Some(note) = note {
        lines = lines.push(muted(text::caption(note)));
    }
    lines.into()
}

fn empty_state<'a>() -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    card(
        widget::column::with_capacity(2)
            .spacing(spacing.space_xxs)
            .push(text::body(crate::fl!("no-contexts-yet")))
            .push(muted(text::caption(crate::fl!(
                "add-one-for-the-kind-of-thing"
            )))),
        false,
    )
}

/// One context's card: the name, what it carries, and the row of actions.
fn context_card<'a>(context: &'a DictationContext, app: &'a AppModel) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let state = &app.contexts;
    let is_active = state.active.as_deref() == Some(context.id.as_str());
    let confirming = state.confirming_delete.as_deref() == Some(context.id.as_str());

    let mut title_row = row![text::heading(context.display_name())]
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center);
    if is_active {
        title_row = title_row.push(active_badge());
    }
    title_row = title_row.push(horizontal_space());

    // "Use" is the primary action on a card that is not in force; on the one
    // that is, there is nothing to press, so the badge above says so instead.
    if !is_active {
        title_row = title_row.push(button::standard(crate::fl!("use")).on_press(
            Message::Contexts(ContextsMessage::Activate(Some(context.id.clone()))),
        ));
    }
    let edit = button::standard(crate::fl!("edit"))
        .on_press(Message::Contexts(ContextsMessage::Edit(context.id.clone())));
    // The first card's is the one `just screenshots` opens.
    let first = state.items.first().is_some_and(|c| c.id == context.id);
    title_row = title_row.push(if first {
        scene::tag(Target::EditContext, edit)
    } else {
        edit.into()
    });
    title_row = title_row.push(delete_control(context, confirming));

    let mut body = widget::column::with_capacity(4)
        .spacing(spacing.space_xxs)
        .width(Length::Fill)
        .push(title_row);

    body = body.push(muted(text::caption(summary_line(context))));

    if !context.vocabulary.is_empty() {
        body = body.push(plain_divider());
        body = body.push(muted(text::caption(preview_terms(&context.vocabulary))));
    }

    card(body, is_active)
}

/// Delete, as one button that arms and a pair that confirms.
///
/// Two presses rather than one, because a vocabulary is a list someone typed by
/// hand and an undo does not exist. The confirm replaces the button in place
/// instead of opening a dialog: the row being deleted stays visible, which is
/// the thing a dialog covers up.
fn delete_control(context: &DictationContext, confirming: bool) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    if confirming {
        row![
            button::text(crate::fl!("cancel"))
                .on_press(Message::Contexts(ContextsMessage::DeleteCancelled)),
            button::destructive(crate::fl!("delete")).on_press(Message::Contexts(
                ContextsMessage::DeleteConfirmed(context.id.clone())
            )),
        ]
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center)
        .into()
    } else {
        button::icon(icons::phosphor_handle(icons::X))
            .class(cosmic::theme::Button::Text)
            .on_press(Message::Contexts(ContextsMessage::DeleteRequested(
                context.id.clone(),
            )))
            .into()
    }
}

/// The "In use" marker on the active card.
fn active_badge<'a>() -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    widget::container(text::caption(crate::fl!("in-use")))
        .padding([0, spacing.space_xxs])
        .class(cosmic::theme::Container::custom(|theme| {
            let cosmic = theme.cosmic();
            cosmic::iced::widget::container::Style {
                text_color: Some(cosmic.accent_text_color().into()),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_xs.into(),
                    width: 1.0,
                    color: cosmic.accent_color().into(),
                },
                ..Default::default()
            }
        }))
        .into()
}

/// "Use no context", as a row under the list.
///
/// A separate affordance because "none" is not a context and putting it in the
/// list as a fake row would make it deletable and editable, which it is not.
fn use_none_row<'a>(already_none: bool) -> Element<'a, Message> {
    let label = if already_none {
        text::caption(crate::fl!("no-context-is-in-use"))
    } else {
        text::caption(crate::fl!("dictating-something-these-do-not-cover"))
    };
    let mut r = row![muted(label), horizontal_space()]
        .spacing(cosmic::theme::spacing().space_xs)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    if !already_none {
        r = r.push(
            button::text(crate::fl!("use-none"))
                .on_press(Message::Contexts(ContextsMessage::Activate(None))),
        );
    }
    r.into()
}

/// "12 terms · a prompt", or whichever halves this context has.
fn summary_line(context: &DictationContext) -> String {
    let mut parts = Vec::new();
    match context.vocabulary.len() {
        0 => {}
        1 => parts.push(crate::fl!("count-1-term")),
        n => parts.push(crate::fl!("terms-count", n = (n).to_string())),
    }
    if !context.prompt.trim().is_empty() {
        parts.push(crate::fl!("a-prompt"));
    }
    if parts.is_empty() {
        return crate::fl!("empty-nothing-is-sent-for-this-one");
    }
    parts.join(" \u{b7} ")
}

/// The first few terms, so a card is recognizable without opening it.
fn preview_terms(vocabulary: &[String]) -> String {
    const SHOWN: usize = 6;
    let head = vocabulary
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match vocabulary.len().saturating_sub(SHOWN) {
        0 => head,
        rest => crate::fl!(
            "context-more-terms",
            head = head.as_str(),
            rest = (rest).to_string()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{preview_terms, summary_line};
    use super_voice_shared::models::contexts::DictationContext;

    fn context(prompt: &str, vocabulary: &[&str]) -> DictationContext {
        DictationContext {
            id: "coding".to_string(),
            name: "Coding".to_string(),
            prompt: prompt.to_string(),
            vocabulary: vocabulary.iter().map(|t| (*t).to_string()).collect(),
        }
    }

    /// A context with neither half is not an error, but it does nothing — and
    /// the card is the only place that can say so.
    #[test]
    fn the_summary_names_the_halves_a_context_actually_has() {
        assert_eq!(
            summary_line(&context("Be terse.", &["kubectl", "rebase"])),
            "\u{2068}2\u{2069} terms \u{b7} a prompt"
        );
        assert_eq!(summary_line(&context("", &["kubectl"])), "1 term");
        assert_eq!(summary_line(&context("Be terse.", &[])), "a prompt");
        assert_eq!(
            summary_line(&context("   ", &[])),
            "Empty \u{2014} nothing is sent for this one.",
            "whitespace is not a prompt"
        );
    }

    #[test]
    fn the_preview_stops_at_six_terms_and_counts_the_rest() {
        let few = ["a", "b", "c"];
        assert_eq!(preview_terms(&few.map(String::from)), "a, b, c");

        let many: Vec<String> = (1..=9).map(|n| n.to_string()).collect();
        assert_eq!(
            preview_terms(&many),
            "\u{2068}1, 2, 3, 4, 5, 6\u{2069} and \u{2068}3\u{2069} more"
        );
    }
}
