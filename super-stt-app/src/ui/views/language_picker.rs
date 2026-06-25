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
    let per_model = app.language_picker_per_model;

    // Build the candidate (tag, label) list for the active mode.
    let mut rows: Vec<(Option<String>, String)> = Vec::new();
    if per_model {
        rows.push((None, "Automatic".to_string())); // clear → DELETE
        rows.push((Some("auto".to_string()), friendly_name("auto")));
        if let Some(block) = &app.active_model_language
            && let Some(arr) = block.get("supported").and_then(|v| v.as_array())
        {
            for tag in arr.iter().filter_map(|v| v.as_str()) {
                rows.push((Some(tag.to_string()), friendly_name(tag)));
            }
        }
    } else {
        rows.push((None, "No preference".to_string())); // clear → DELETE
        rows.push((Some("auto".to_string()), friendly_name("auto")));
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
        let msg = if per_model {
            Message::ActiveModelLanguageSelected(tag.clone())
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
