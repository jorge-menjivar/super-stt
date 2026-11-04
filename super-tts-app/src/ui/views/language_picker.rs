// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget::{self, button, column, scrollable, text_input};

use crate::core::app::AppModel;
use crate::ui::languages::friendly_name;
use crate::ui::messages::{LanguageMessage, Message};

/// The language search sheet, in either of its two modes.
///
/// Both lists are the daemon's, never this app's. Which tags a model answers to
/// — whether it wants `en` or `en-US` — is a rule only the daemon's resolver
/// knows, and the global setting's vocabulary is the union of what the installed
/// models can speak, so it grows and shrinks as backends come and go. The
/// hard-coded table that used to fill the global half offered tags no model
/// served and, more quietly, never learned about the ones a freshly installed
/// backend added. `auto` is pinned rather than taken from either list, because
/// it is a control and not a language, and a search query must not be able to
/// filter it away.
pub fn sheet(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let q = app.language.language_picker_query.to_lowercase();

    // Build the candidate (tag, label) list for the active mode. Special
    // entries stay pinned at the top; the language entries are sorted
    // alphabetically by display name.
    let mut pinned: Vec<(Option<String>, String)> = Vec::new();
    let mut langs: Vec<(Option<String>, String)> = Vec::new();
    if let Some((ref src, ref mdl)) = app.language.language_picker_target {
        // Per-model sheet.
        pinned.push((None, "Follow global".to_string())); // clear → DELETE
        pinned.push((Some("auto".to_string()), friendly_name("auto"))); // "Auto-detect"
        // The tags this model can be pinned to — but only when the held list
        // belongs to this exact (source, model) pair (stale-list guard). An
        // empty list is a real answer for a monolingual model, and leaves the
        // sheet with nothing but the two pinned controls.
        if app.language.model_language_for.as_ref() == Some(&(src.clone(), mdl.clone())) {
            for tag in &app.language.model_languages {
                if tag.eq_ignore_ascii_case("auto") {
                    continue; // already pinned as "Auto-detect"
                }
                langs.push((Some(tag.clone()), friendly_name(tag)));
            }
        }
    } else {
        // Global sheet — "Auto-detect" only; the unset state is reached by not
        // choosing anything, so there is no explicit "No preference" entry.
        pinned.push((Some("auto".to_string()), friendly_name("auto"))); // "Auto-detect"
        for tag in &app.language.primary_languages {
            if tag.eq_ignore_ascii_case("auto") {
                continue; // the daemon always includes it; it is pinned above
            }
            langs.push((Some(tag.clone()), friendly_name(tag)));
        }
    }
    langs.sort_by(|a, b| a.1.cmp(&b.1));
    // The pinned controls always show; only the language list is narrowed by the
    // search query, so the user can never filter away "Auto-detect" / "Follow
    // global".
    let rows: Vec<(Option<String>, String)> = pinned
        .into_iter()
        .chain(langs.into_iter().filter(|(tag, label)| {
            if q.is_empty() {
                return true;
            }
            let hay = format!("{label} {}", tag.as_deref().unwrap_or("")).to_lowercase();
            hay.contains(&q)
        }))
        .collect();

    // Search field pinned at the top.
    let search = text_input("Search languages…", &app.language.language_picker_query)
        .on_input(|s| Message::Language(LanguageMessage::LanguagePickerQueryChanged(s)))
        .width(Length::Fill);

    let mut list = column::with_capacity(rows.len()).spacing(spacing.space_xxs);
    for (tag, label) in rows {
        let msg = if let Some((ref src, ref mdl)) = app.language.language_picker_target {
            Message::Language(LanguageMessage::ModelLanguageSelected {
                source: src.clone(),
                model: mdl.clone(),
                choice: tag,
            })
        } else {
            Message::Language(LanguageMessage::PrimaryLanguageSelected(tag))
        };
        list = list.push(button::text(label).width(Length::Fill).on_press(msg));
    }

    widget::column::with_capacity(2)
        .spacing(spacing.space_s)
        .push(search)
        .push(scrollable(list).height(Length::Fill))
        .width(Length::Fill)
        .into()
}
