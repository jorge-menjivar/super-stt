// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, settings, space::horizontal as horizontal_space, text};
use cosmic::{Apply, Element};
use super_voice_shared::models::notification_method::NotificationMethod;
use super_voice_shared::theme::AudioTheme;

use super::common::{error_banner, page_layout};
use crate::ui::messages::{LanguageMessage, Message, NotificationMethodMessage, RecordingMessage};

/// Customization page: what both features share — audio feedback, its theme
/// and volume, the primary language, and how failures are reported.
pub fn page<'a>(
    audio_themes: &'a [AudioTheme],
    selected_audio_theme: &'a AudioTheme,
    volume: u8,
    app_primary_language: Option<&'a str>,
    notification_method: NotificationMethod,
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
        .map(|t| crate::i18n::audio_theme_name(**t))
        .collect();
    let selected_index = non_silent_themes
        .iter()
        .position(|t| *t == selected_audio_theme);

    let mut section = settings::section().title(crate::fl!("audio")).add(
        settings::item::builder(crate::fl!("audio-feedback"))
            .description(crate::fl!("play-sounds-when-recording-or-speech-starts"))
            .control(
                cosmic::widget::toggler(audio_enabled)
                    .on_toggle(|b| Message::Recording(RecordingMessage::AudioFeedbackToggled(b))),
            ),
    );

    if audio_enabled {
        let non_silent_clone: Vec<AudioTheme> =
            non_silent_themes.iter().copied().copied().collect();
        let theme_control: Element<'a, Message> = if non_silent_themes.is_empty() {
            text::caption(crate::fl!("loading-themes")).into()
        } else {
            widget::dropdown(theme_names, selected_index, move |index| {
                if let Some(&theme) = non_silent_clone.get(index) {
                    Message::Recording(RecordingMessage::AudioThemeSelected(theme))
                } else {
                    Message::Recording(RecordingMessage::AudioThemeSelected(AudioTheme::Classic))
                }
            })
            .into()
        };

        // Drag ticks update the local value; the daemon POST fires once on
        // release (Tier 1 #19), so a single drag isn't hundreds of set_volume
        // requests.
        let slider = widget::slider(0..=100, volume, |v| {
            Message::Recording(RecordingMessage::VolumeChanged(v))
        })
        .on_release(Message::Recording(RecordingMessage::VolumeCommit))
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
                settings::item::builder(crate::fl!("theme"))
                    .description(crate::fl!("sound-theme-for-recording-and-speech"))
                    .control(theme_control),
            )
            .add(
                settings::item::builder(crate::fl!("volume"))
                    .flex_control(volume_control)
                    .align_items(Alignment::Center),
            );
    }

    let lang_label = app_primary_language.map_or_else(
        || crate::fl!("automatic"),
        crate::ui::languages::friendly_name,
    );
    let language_section = settings::section().title(crate::fl!("language")).add(
        settings::item::builder(crate::fl!("primary-language"))
            .description(crate::fl!("default-language-for-models-that-support-it"))
            .control(
                widget::button::standard(lang_label).on_press(Message::Language(
                    LanguageMessage::OpenLanguagePicker { model: None },
                )),
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
    blocks.push(notifications_section(notification_method));

    page_layout(crate::fl!("customization"), settings::view_column(blocks))
}

/// Notifications section: how either feature's failures reach the user. One
/// setting for both, so it is here with the others they share.
fn notifications_section(notification_method: NotificationMethod) -> Element<'static, Message> {
    let methods = [
        NotificationMethod::Auto,
        NotificationMethod::Desktop,
        NotificationMethod::Typed,
        NotificationMethod::Off,
    ];
    let method_names: Vec<String> = methods
        .iter()
        .map(|m| crate::i18n::notification_method_name(*m))
        .collect();
    let selected_index = methods.iter().position(|m| *m == notification_method);

    settings::section()
        .title(crate::fl!("notifications"))
        .add(
            settings::item::builder(crate::fl!("failure-notices"))
                .description(crate::fl!("how-recording-and-speech-failures-are-reported"))
                .control(widget::dropdown(
                    method_names,
                    selected_index,
                    move |index| {
                        Message::NotificationMethod(NotificationMethodMessage::Changed(
                            methods[index],
                        ))
                    },
                )),
        )
        .into()
}
