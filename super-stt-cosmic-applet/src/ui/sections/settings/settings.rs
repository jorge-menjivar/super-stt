// SPDX-License-Identifier: GPL-3.0-only
use cosmic::{
    applet::padded_control,
    iced::{
        widget::{column, row, slider},
        Alignment, Length,
    },
    theme,
    widget::{
        divider, segmented_button::SingleSelectModel, segmented_control, text, toggler, Space,
    },
    Apply, Element,
};
use super_stt_shared::theme::AudioTheme;

use crate::{
    app::Message,
    config::AppletConfig,
    models::theme::ThemeConfig,
    ui::{
        components::common::revealer,
        sections::settings::components::visualization_theme::{
            create_visualization_color_selector, create_visualization_theme_selector,
        },
    },
    IsOpen,
};

pub fn create_audio_theme_selector<'a>(
    selected_theme: AudioTheme,
    is_open: &IsOpen,
    available_themes: &[AudioTheme],
) -> Element<'a, Message> {
    // Use provided themes if available, otherwise fallback to all themes
    let audio_themes = if available_themes.is_empty() {
        AudioTheme::all_themes()
    } else {
        available_themes.to_vec()
    };

    let options: Vec<(String, String)> = audio_themes
        .into_iter()
        .map(|theme| (theme.to_string(), theme.pretty_name()))
        .collect();

    revealer(
        *is_open == IsOpen::AudioTheme,
        "Audio Theme".to_string(),
        selected_theme.pretty_name().clone(),
        &options,
        Message::RevealerToggle(IsOpen::AudioTheme),
        |theme_str| Message::SetAudioTheme(theme_str.parse::<AudioTheme>().unwrap_or_default()),
    )
    .apply(Element::from)
}

pub fn create_applet_settings_section<'a>(
    config: &AppletConfig,
    theme_config: &ThemeConfig,
    is_open: &IsOpen,
    icon_alignment_model: &'a SingleSelectModel,
    theme_selector_model: &'a SingleSelectModel,
    selected_theme_for_config: bool,
    available_audio_themes: &[AudioTheme],
) -> Element<'a, Message> {
    let spacing = theme::active().cosmic().spacing;

    let mut settings_column = column![
        create_audio_theme_selector(theme_config.audio_theme, is_open, available_audio_themes),
        padded_control(divider::horizontal::default())
            .padding([0, spacing.space_s])
            .apply(Element::from),
        // Show visualizations toggle
        padded_control(
            row![
                text::body("Show Visualization"),
                Space::new(Length::Fill, Length::Shrink),
                toggler(config.ui.show_visualization).on_toggle(Message::SetShowVisualizations)
            ]
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center),
        ),
    ]
    .spacing(spacing.space_xs)
    .width(Length::Fill);

    // Visualization size slide (only show if the visualization is enabled)
    if config.ui.show_visualization {
        // Width slider
        settings_column = settings_column.push(
            column![
                padded_control(
                    column![
                        text::body("Visualization Size"),
                        row![
                            text::caption(format!("{}px", config.ui.applet_width)),
                            slider(60..=300, config.ui.applet_width, Message::SetAppletWidth)
                                .width(Length::Fill)
                        ]
                        .spacing(spacing.space_xs)
                        .align_y(Alignment::Center),
                    ]
                    .spacing(spacing.space_xxs)
                    .apply(Element::from)
                ),
                create_visualization_theme_selector(&theme_config.visualization_theme, is_open),
                create_visualization_color_selector(
                    &theme_config.visualization_color_config,
                    is_open,
                    theme_selector_model,
                    selected_theme_for_config
                )
            ]
            .spacing(spacing.space_xxs)
            .apply(Element::from),
        );
    }

    settings_column = settings_column.push(
        padded_control(divider::horizontal::default())
            .padding([0, spacing.space_s])
            .apply(Element::from),
    );

    settings_column = settings_column.push(
        // Show icon toggle
        padded_control(
            row![
                text::body("Show Icon"),
                Space::new(Length::Fill, Length::Shrink),
                toggler(config.ui.show_icon).on_toggle(Message::SetShowIcon)
            ]
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center),
        ),
    );

    // Icon alignment selector (only show if icon is enabled)
    if config.ui.show_icon {
        settings_column = settings_column.push(
            padded_control(
                column![
                    text::body("Icon Position"),
                    segmented_control::horizontal(icon_alignment_model)
                        .on_activate(Message::SetIconAlignmentEntity)
                ]
                .spacing(spacing.space_xxs)
                .apply(Element::from),
            )
            .apply(Element::from),
        );
    }

    settings_column.apply(Element::from)
}
