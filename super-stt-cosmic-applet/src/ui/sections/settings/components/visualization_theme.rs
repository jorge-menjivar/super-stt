// SPDX-License-Identifier: GPL-3.0-only
use crate::{
    IsOpen,
    app::Message,
    models::theme::{VisualizationColor, VisualizationColorConfig, VisualizationTheme},
    ui::components::{
        color_buttons::{create_color_button, create_system_accent_button},
        common::{revealer, revealer_head},
    },
};
use cosmic::{
    Apply, Element, Theme,
    applet::padded_control,
    iced::{
        Length,
        widget::{column, row},
    },
    iced_widget::Row,
    theme,
    widget::{Space, segmented_button::SingleSelectModel, segmented_control, text},
};

pub fn create_visualization_theme_selector<'a>(
    selected_theme: &VisualizationTheme,
    is_open: &IsOpen,
) -> Element<'a, Message> {
    let vis_themes = vec![
        VisualizationTheme::Pulse,
        VisualizationTheme::BottomEqualizer,
        VisualizationTheme::CenteredEqualizer,
        VisualizationTheme::Waveform,
    ];

    let options: Vec<(String, String)> = vis_themes
        .into_iter()
        .map(|theme| (theme.to_string(), theme.pretty_name()))
        .collect();

    revealer(
        *is_open == IsOpen::VisualizationTheme,
        "Visualization Style".to_string(),
        selected_theme.pretty_name(),
        &options,
        Message::RevealerToggle(IsOpen::VisualizationTheme),
        |theme_str| Message::SetVisualizationTheme(VisualizationTheme::from_str(&theme_str)),
    )
    .apply(Element::from)
}

pub fn create_visualization_color_selector<'a>(
    color_config: &VisualizationColorConfig,
    is_open: &IsOpen,
    theme_selector_model: &'a SingleSelectModel,
    selected_theme_for_config: bool,
) -> Element<'a, Message> {
    let spacing = theme::active().cosmic().spacing;

    if *is_open == IsOpen::VisualizationColors {
        column![
            revealer_head(
                true,
                "Visualization Color".to_string(),
                "Theme-Aware Color".to_string(),
                Message::RevealerToggle(IsOpen::VisualizationColors)
            ),
            // Theme selector
            padded_control(
                column![
                    text::caption("Configure Colors For:"),
                    segmented_control::horizontal(theme_selector_model)
                        .on_activate(Message::SetColorThemeEntity)
                ]
                .spacing(spacing.space_xxs)
                .apply(Element::from)
            )
            .padding([8, 48]),
            // Theme-specific color controls
            create_theme_color_section(
                selected_theme_for_config,
                &color_config.get_color(selected_theme_for_config)
            ),
        ]
        .width(Length::Fill)
        .apply(Element::from)
    } else {
        column![revealer_head(
            false,
            "Visualization Color".to_string(),
            "Theme-Aware Color".to_string(),
            Message::RevealerToggle(IsOpen::VisualizationColors)
        )]
        .apply(Element::from)
    }
}

fn create_colors_row<'a>(
    colors: &[VisualizationColor],
    selected_theme_is_dark: bool,
    current_selected_color: &VisualizationColor,
) -> Row<'a, Message, Theme> {
    let mut light_colors_row = row![];
    for (index, color) in colors.iter().enumerate() {
        light_colors_row = light_colors_row.push(create_color_button(
            color.to_color(),
            color,
            Message::SetVisualizationColor(color.clone(), selected_theme_is_dark),
            20.0,
            current_selected_color == color,
        ));
        if index != colors.len() - 1 {
            light_colors_row = light_colors_row.push(Space::new().width(Length::Fill));
        }
    }
    light_colors_row
}

fn create_theme_color_section<'a>(
    selected_theme_is_dark: bool,
    current_selected_color: &VisualizationColor,
) -> Element<'a, Message> {
    let current_theme = theme::active();
    let spacing = current_theme.cosmic().spacing;
    let current_cosmic_theme = current_theme.cosmic();

    // Light colors that work well in light themes
    let light_colors = vec![
        VisualizationColor::White,
        VisualizationColor::Gray,
        VisualizationColor::Blue,
        VisualizationColor::Green,
        VisualizationColor::Orange,
        VisualizationColor::Purple,
        VisualizationColor::Violet,
        VisualizationColor::Red,
        VisualizationColor::Cyan,
        VisualizationColor::Pink,
    ];

    // Darker/deeper colors that work well in dark themes
    let dark_colors = vec![
        VisualizationColor::Black,
        VisualizationColor::DarkGray,
        VisualizationColor::DarkBlue,
        VisualizationColor::DarkGreen,
        VisualizationColor::DarkOrange,
        VisualizationColor::DarkPurple,
        VisualizationColor::DarkViolet,
        VisualizationColor::DarkRed,
        VisualizationColor::DarkCyan,
        VisualizationColor::DarkPink,
    ];

    let pastel_colors = vec![
        VisualizationColor::PastelBlue,
        VisualizationColor::PastelGreen,
        VisualizationColor::PastelOrange,
        VisualizationColor::PastelPurple,
        VisualizationColor::PastelRed,
        VisualizationColor::PastelCyan,
        VisualizationColor::PastelPink,
        VisualizationColor::PastelYellow,
        VisualizationColor::PastelMagenta,
        VisualizationColor::PastelLavender,
    ];

    padded_control(
        column![
            // Color grid section label
            text::caption("Default Colors").width(Length::Fill),
            // Dedicated System Accent row - full width button
            create_system_accent_button(
                selected_theme_is_dark,
                current_cosmic_theme,
                current_selected_color
            ),
            // Color grid section label
            text::caption("Custom Colors").width(Length::Fill),
            // Top row of colors
            create_colors_row(
                &light_colors,
                selected_theme_is_dark,
                current_selected_color
            )
            .spacing(0u16),
            // Row of dark colors
            create_colors_row(&dark_colors, selected_theme_is_dark, current_selected_color),
            // Row of pastel colors
            create_colors_row(
                &pastel_colors,
                selected_theme_is_dark,
                current_selected_color
            )
            .spacing(0u16)
        ]
        .spacing(spacing.space_xxs),
    )
    .padding([8, 48])
    .apply(Element::from)
}
