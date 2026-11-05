// SPDX-License-Identifier: GPL-3.0-only
//! The editor sheet: one context's name, prompt and vocabulary.
//!
//! The vocabulary is a column of one input per term rather than a textarea, and
//! that is the deliberate part. A textarea makes the boundary between terms a
//! parsing question — is `Menjivar, Jorge` one term or two? — and makes a
//! single term in the middle of a long list awkward to fix. Discrete inputs
//! make the boundaries unrepresentable-wrong and every term directly editable.
//!
//! The textarea's one real advantage is kept: a pasted multi-line blob splits
//! across rows instead of landing as one term with newlines in it.

use cosmic::Element;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, space::horizontal as horizontal_space, text, text_input};

use crate::core::app::AppModel;
use crate::state::contexts::ContextDraft;
use crate::ui::icons;
use crate::ui::messages::{ContextsMessage, Message};
use crate::ui::views::common;

use super::surface::muted;

/// The drawer's title: what the user is doing, not what the sheet contains.
#[must_use]
pub fn editor_title(draft: &ContextDraft) -> String {
    if draft.is_new {
        "New context".to_string()
    } else {
        let name = draft.name.trim();
        if name.is_empty() {
            "Edit context".to_string()
        } else {
            format!("Edit {name}")
        }
    }
}

/// The editor body. Returns nothing when no draft is open, which is what scopes
/// the sheet to the page that opened it.
pub fn editor_sheet(app: &AppModel) -> Option<Element<'_, Message>> {
    let draft = app.contexts.draft.as_ref()?;
    let spacing = cosmic::theme::spacing();

    let mut body = widget::column::with_capacity(8)
        .spacing(spacing.space_m)
        .width(Length::Fill);

    if let Some(message) = &draft.error {
        body = body.push(common::error_banner(message));
    }

    body = body.push(name_field(draft));
    body = body.push(prompt_field(draft));
    body = body.push(vocabulary_field(draft));
    body = body.push(actions(draft));

    Some(body.into())
}

fn name_field(draft: &ContextDraft) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    widget::column::with_capacity(2)
        .spacing(spacing.space_xxs)
        .push(text::heading("Name"))
        .push(
            widget::text_input("Coding", &draft.name)
                .on_input(|v| Message::Contexts(ContextsMessage::NameChanged(v))),
        )
        .into()
}

/// The prompt: free text for a model that follows instructions.
///
/// Worth saying that most do not, because a user who writes a paragraph of
/// instructions and sees no change has been misled by the field rather than by
/// the model.
fn prompt_field(draft: &ContextDraft) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    widget::column::with_capacity(3)
        .spacing(spacing.space_xxs)
        .push(text::heading("Prompt"))
        .push(muted(text::caption(
            "Instructions, for a model that follows them. Most speech models do not \
             \u{2014} for those, the words below are what helps.",
        )))
        .push(
            widget::text_editor(&draft.prompt)
                .height(Length::Fixed(120.0))
                .on_action(|a| Message::Contexts(ContextsMessage::PromptAction(a))),
        )
        .into()
}

/// The vocabulary: one input per term, Enter for the next one.
fn vocabulary_field(draft: &ContextDraft) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let filled = draft
        .terms
        .iter()
        .filter(|(_, t)| !t.trim().is_empty())
        .count();

    let rows: Vec<Element<'_, Message>> = draft
        .terms
        .iter()
        .enumerate()
        .map(|(index, (id, term))| term_row(index, id, term, draft.terms.len()))
        .collect();

    widget::column::with_capacity(4)
        .spacing(spacing.space_xxs)
        .push(
            row![
                text::heading("Words to listen for"),
                horizontal_space(),
                muted(text::caption(count_label(filled))),
            ]
            .align_y(Alignment::Center)
            .width(Length::Fill),
        )
        .push(muted(text::caption(
            "Names, jargon, anything the model keeps mishearing. Press Enter for the \
             next one.",
        )))
        .push(
            widget::scrollable(column(rows).spacing(spacing.space_xxs).width(Length::Fill))
                .height(Length::Fixed(220.0)),
        )
        .into()
}

/// One term's row: the input, and a clear button for every row but the last.
///
/// The trailing row has no clear button because it is the empty one the next
/// term goes in — removing it would leave nowhere to type, and it costs nothing
/// to keep since blanks are dropped on save.
fn term_row<'a>(
    index: usize,
    id: &'a widget::Id,
    term: &'a str,
    total: usize,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let input = text_input("", term)
        .id(id.clone())
        .on_input(move |value| Message::Contexts(ContextsMessage::TermChanged { index, value }))
        .on_submit(move |_| Message::Contexts(ContextsMessage::TermSubmitted(index)))
        .width(Length::Fill);

    let mut r = row![input]
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    if total > 1 {
        r = r.push(
            button::icon(icons::phosphor_handle(icons::X))
                .class(cosmic::theme::Button::Text)
                .on_press(Message::Contexts(ContextsMessage::TermRemoved(index))),
        );
    }
    r.into()
}

fn actions(draft: &ContextDraft) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let save = if draft.saving {
        button::suggested("Saving\u{2026}")
    } else {
        button::suggested("Save").on_press(Message::Contexts(ContextsMessage::Save))
    };
    row![
        horizontal_space(),
        button::standard("Cancel").on_press(Message::Contexts(ContextsMessage::CancelEdit)),
        save,
    ]
    .spacing(spacing.space_xs)
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}

/// "0 words" / "1 word" / "12 words".
fn count_label(filled: usize) -> String {
    match filled {
        1 => "1 word".to_string(),
        n => format!("{n} words"),
    }
}

#[cfg(test)]
mod tests {
    use super::{count_label, editor_title};
    use crate::state::contexts::ContextDraft;
    use super_stt_shared::models::contexts::DictationContext;

    #[test]
    fn the_title_says_what_the_user_is_doing() {
        let new = ContextDraft::creating("context-1".to_string());
        assert_eq!(editor_title(&new), "New context");

        let existing = ContextDraft::editing(&DictationContext {
            id: "coding".to_string(),
            name: "Coding".to_string(),
            ..DictationContext::default()
        });
        assert_eq!(editor_title(&existing), "Edit Coding");

        // A name cleared mid-edit must not render as "Edit ".
        let mut unnamed = existing;
        unnamed.name = "  ".to_string();
        assert_eq!(editor_title(&unnamed), "Edit context");
    }

    #[test]
    fn the_count_is_singular_at_one() {
        assert_eq!(count_label(0), "0 words");
        assert_eq!(count_label(1), "1 word");
        assert_eq!(count_label(12), "12 words");
    }
}
