// SPDX-License-Identifier: GPL-3.0-only
use cosmic::{
    Apply, Element,
    applet::padded_control,
    iced::{
        Alignment, Length,
        widget::{column, row, slider},
    },
    theme,
    widget::{
        Space, divider, segmented_button::SingleSelectModel, segmented_control, text, toggler,
    },
};

use crate::{
    app::Message,
    config::AppletConfig,
    models::state::IsOpen,
    models::theme::ThemeConfig,
    ui::sections::settings::components::visualization_theme::{
        create_visualization_color_selector, create_visualization_theme_selector,
        create_working_animation_selector,
    },
};

pub fn create_applet_settings_section<'a>(
    config: &AppletConfig,
    theme_config: &ThemeConfig,
    is_open: &IsOpen,
    icon_alignment_model: &'a SingleSelectModel,
    theme_selector_model: &'a SingleSelectModel,
    selected_theme_for_config: bool,
) -> Element<'a, Message> {
    let spacing = theme::active().cosmic().spacing;

    let mut settings_column = column![
        // Show visualizations toggle
        padded_control(
            row![
                text::body("Show Visualization"),
                Space::new().width(Length::Fill),
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
                create_working_animation_selector(config.visualization.working_animation, is_open),
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
                Space::new().width(Length::Fill),
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
