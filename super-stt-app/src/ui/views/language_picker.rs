// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget::{self, button, column, scrollable, text_input};

use crate::core::app::AppModel;
use crate::ui::languages::friendly_name;
use crate::ui::messages::{LanguageMessage, Message};

pub fn sheet(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let q = app.language.language_picker_query.to_lowercase();

    // Build the candidate (tag, label) list for the active mode. Special
    // entries stay pinned at the top; the language entries are sorted
    // alphabetically by display name.
    let mut pinned: Vec<(Option<String>, String)> = Vec::new();
    let mut langs: Vec<(Option<String>, String)> = Vec::new();
    if let Some((stage, ref src, ref mdl)) = app.language.language_picker_target {
        // Per-model sheet.
        pinned.push((None, "Follow global".to_string())); // clear → DELETE
        // What the daemon will accept for this model, from
        // `/language/list` — `auto` included, which is why it is not pinned
        // here the way the global sheet pins it. Offering a general BCP-47 list
        // instead would put tags in front of the user that the setter refuses,
        // discoverable only by choosing one.
        for tag in app
            .language
            .model_languages
            .offered(stage, src, mdl)
            .unwrap_or_default()
        {
            langs.push((Some(tag.clone()), friendly_name(tag)));
        }
    } else {
        // Global sheet — the daemon's own vocabulary, `auto` included, from
        // `/settings/language/list`. The unset state is reached by not choosing
        // anything, so there is no explicit "No preference" entry.
        for tag in &app.language.global_offers {
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
        let msg = if let Some((stage, ref src, ref mdl)) = app.language.language_picker_target {
            Message::Language(LanguageMessage::ModelLanguageSelected {
                stage,
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
