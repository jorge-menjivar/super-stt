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

    // Build the candidate (tag, label) list for the active mode.
    let mut rows: Vec<(Option<String>, String)> = Vec::new();
    if let Some((ref src, ref mdl)) = app.language_picker_target {
        // Per-model sheet.
        rows.push((None, "Follow global".to_string())); // clear → DELETE
        rows.push((Some("auto".to_string()), friendly_name("auto"))); // "Auto-detect"
        // Supported languages from the resolution block — but only when the
        // block belongs to this exact (source, model) pair (stale-block guard).
        if app.model_language_for.as_ref() == Some(&(src.clone(), mdl.clone()))
            && let Some(block) = &app.model_language
            && let Some(arr) = block.get("supported").and_then(|v| v.as_array())
        {
            for tag in arr.iter().filter_map(|v| v.as_str()) {
                rows.push((Some(tag.to_string()), friendly_name(tag)));
            }
        }
    } else {
        // Global sheet.
        rows.push((None, "No preference".to_string())); // clear → DELETE
        rows.push((Some("auto".to_string()), friendly_name("auto"))); // "Auto-detect"
        for (tag, name) in GLOBAL_LANGUAGES {
            rows.push((Some((*tag).to_string()), (*name).to_string()));
        }
    }

    // Search field pinned at the top.
    let search = text_input("Search languages…", &app.language_picker_query)
        .on_input(Message::LanguagePickerQueryChanged)
        .width(Length::Fill);

    let mut list = column::with_capacity(rows.len()).spacing(spacing.space_xxs);
    for (tag, label) in rows {
        let hay = format!("{label} {}", tag.as_deref().unwrap_or("")).to_lowercase();
        if !q.is_empty() && !hay.contains(&q) {
            continue;
        }
        let msg = if let Some((ref src, ref mdl)) = app.language_picker_target {
            Message::ModelLanguageSelected {
                source: src.clone(),
                model: mdl.clone(),
                choice: tag.clone(),
            }
        } else {
            Message::PrimaryLanguageSelected(tag.clone())
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
