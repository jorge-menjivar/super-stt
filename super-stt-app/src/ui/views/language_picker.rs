// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget::{self, button, column, scrollable, text_input};

use crate::core::app::AppModel;
use crate::ui::languages::{GLOBAL_LANGUAGES, friendly_name};
use crate::ui::messages::Message;

pub fn sheet(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let q = app.language_picker_query.to_lowercase();

    // Build the candidate (tag, label) list for the active mode. Special
    // entries stay pinned at the top; the language entries are sorted
    // alphabetically by display name.
    let mut pinned: Vec<(Option<String>, String)> = Vec::new();
    let mut langs: Vec<(Option<String>, String)> = Vec::new();
    if let Some((ref src, ref mdl)) = app.language_picker_target {
        // Per-model sheet.
        pinned.push((None, "Follow global".to_string())); // clear → DELETE
        pinned.push((Some("auto".to_string()), friendly_name("auto"))); // "Auto-detect"
        // Supported languages from the resolution block — but only when the
        // block belongs to this exact (source, model) pair (stale-block guard).
        if app.model_language_for.as_ref() == Some(&(src.clone(), mdl.clone()))
            && let Some(block) = &app.model_language
            && let Some(arr) = block.get("supported").and_then(|v| v.as_array())
        {
            for tag in arr.iter().filter_map(|v| v.as_str()) {
                if tag.eq_ignore_ascii_case("auto") {
                    continue; // already pinned as "Auto-detect"
                }
                langs.push((Some(tag.to_string()), friendly_name(tag)));
            }
        }
    } else {
        // Global sheet — "Auto-detect" only; the unset state is reached by not
        // choosing anything, so there is no explicit "No preference" entry.
        pinned.push((Some("auto".to_string()), friendly_name("auto"))); // "Auto-detect"
        for tag in GLOBAL_LANGUAGES {
            langs.push((Some((*tag).to_string()), friendly_name(tag)));
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
    let search = text_input("Search languages…", &app.language_picker_query)
        .on_input(Message::LanguagePickerQueryChanged)
        .width(Length::Fill);

    let mut list = column::with_capacity(rows.len()).spacing(spacing.space_xxs);
    for (tag, label) in rows {
        let msg = if let Some((ref src, ref mdl)) = app.language_picker_target {
            Message::ModelLanguageSelected {
                source: src.clone(),
                model: mdl.clone(),
                choice: tag,
            }
        } else {
            Message::PrimaryLanguageSelected(tag)
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
