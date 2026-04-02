// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::widget::{self, settings, text};
use super_stt_shared::theme::AudioTheme;

use super::common::page_layout;
use crate::ui::messages::Message;

/// Customization page: audio theme selection
pub fn page<'a>(
    audio_themes: &'a [AudioTheme],
    selected_audio_theme: &'a AudioTheme,
) -> Element<'a, Message> {
    let theme_names: Vec<String> = audio_themes.iter().map(AudioTheme::pretty_name).collect();
    let selected_index = audio_themes
        .iter()
        .position(|theme| theme == selected_audio_theme);
    let audio_themes_clone = audio_themes.to_vec();

    let theme_control: Element<'a, Message> = if audio_themes.is_empty() {
        text::caption("Loading themes...").into()
    } else {
        widget::dropdown(theme_names, selected_index, move |index| {
            if let Some(&theme) = audio_themes_clone.get(index) {
                Message::AudioThemeSelected(theme)
            } else {
                Message::AudioThemeSelected(AudioTheme::Classic)
            }
        })
        .into()
    };

    let sections = settings::view_column(vec![
        settings::section()
            .title("Audio")
            .add(
                settings::item::builder("Theme")
                    .description("Sound theme for recording feedback")
                    .control(theme_control),
            )
            .into(),
    ]);

    page_layout("Customization", sections)
}
