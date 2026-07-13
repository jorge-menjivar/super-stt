// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, settings, space::horizontal as horizontal_space, text};
use cosmic::{Apply, Element};
use super_stt_shared::theme::AudioTheme;

use super::common::{error_banner, page_layout};
use crate::ui::messages::Message;

/// Customization page: audio feedback toggle, theme selection, volume, and language.
pub fn page<'a>(
    audio_themes: &'a [AudioTheme],
    selected_audio_theme: &'a AudioTheme,
    volume: u8,
    app_primary_language: Option<&'a str>,
    action_error: Option<&'a str>,
) -> Element<'a, Message> {
    let audio_enabled = *selected_audio_theme != AudioTheme::Silent;

    // Filter Silent out of the dropdown — it's controlled by the toggle
    let non_silent_themes: Vec<&AudioTheme> = audio_themes
        .iter()
        .filter(|t| **t != AudioTheme::Silent)
        .collect();
    let theme_names: Vec<String> = non_silent_themes
        .iter()
        .map(|t| AudioTheme::pretty_name(t))
        .collect();
    let selected_index = non_silent_themes
        .iter()
        .position(|t| *t == selected_audio_theme);

    let mut section = settings::section().title("Audio").add(
        settings::item::builder("Audio Feedback")
            .description("Play sounds when recording starts and stops")
            .control(
                cosmic::widget::toggler(audio_enabled).on_toggle(Message::AudioFeedbackToggled),
            ),
    );

    if audio_enabled {
        let non_silent_clone: Vec<AudioTheme> =
            non_silent_themes.iter().copied().copied().collect();
        let theme_control: Element<'a, Message> = if non_silent_themes.is_empty() {
            text::caption("Loading themes...").into()
        } else {
            widget::dropdown(theme_names, selected_index, move |index| {
                if let Some(&theme) = non_silent_clone.get(index) {
                    Message::AudioThemeSelected(theme)
                } else {
                    Message::AudioThemeSelected(AudioTheme::Classic)
                }
            })
            .into()
        };

        let slider = widget::slider(0..=100, volume, Message::VolumeChanged)
            .width(Length::Fill)
            .apply(widget::container)
            .max_width(250.);

        let volume_control = widget::row::with_capacity(3)
            .align_y(Alignment::Center)
            .push(
                text::body(format!("{volume}%"))
                    .width(Length::Fixed(32.0))
                    .align_x(Alignment::Center),
            )
            .push(horizontal_space().width(8.))
            .push(slider);

        section = section
            .add(
                settings::item::builder("Theme")
                    .description("Sound theme for recording feedback")
                    .control(theme_control),
            )
            .add(
                settings::item::builder("Volume")
                    .flex_control(volume_control)
                    .align_items(Alignment::Center),
            );
    }

    let lang_label = app_primary_language.map_or_else(
        || "Automatic".to_string(),
        crate::ui::languages::friendly_name,
    );
    let language_section = settings::section().title("Language").add(
        settings::item::builder("Primary Language")
            .description("Default transcription language for models that support it")
            .control(
                widget::button::standard(lang_label)
                    .on_press(Message::OpenLanguagePicker { model: None }),
            ),
    );

    // Surface a failed audio-settings save inline (Tier 1 #13) instead of
    // letting the failure flip the whole app to the connection-error page.
    let mut blocks: Vec<Element<'a, Message>> = Vec::new();
    if let Some(message) = action_error {
        blocks.push(error_banner(message));
    }
    blocks.push(section.into());
    blocks.push(language_section.into());

    page_layout("Customization", settings::view_column(blocks))
}
