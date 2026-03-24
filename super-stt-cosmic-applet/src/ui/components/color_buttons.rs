// SPDX-License-Identifier: GPL-3.0-only
use cosmic::{
    iced::{widget::row, Alignment, Color, Length},
    theme,
    widget::{container, mouse_area, text, Space},
    Apply, Element,
};

use crate::{app::Message, models::theme::VisualizationColor};

pub fn create_system_accent_button<'a>(
    selected_theme_is_dark: bool,
    cosmic_theme: &cosmic::cosmic_theme::Theme,
    current_selected_color: &VisualizationColor,
) -> Element<'a, Message> {
    let spacing = theme::active().cosmic().spacing;
    let accent_color = VisualizationColor::SystemAccent.to_color_with_theme(cosmic_theme);
    let is_selected = current_selected_color == &VisualizationColor::SystemAccent;

    // Wrap the system accent button with a tooltip
    cosmic::widget::tooltip(
        // Create a full-width clickable area using mouse_area to avoid button background
        mouse_area(
            container(
                row![
                    // Small accent color preview
                    container(text(""))
                        .width(Length::Fixed(20.0))
                        .height(Length::Fixed(20.0))
                        .style(move |theme| container::Style {
                            background: Some(cosmic::iced::Background::Color(accent_color)),
                            border: cosmic::iced::Border {
                                color: Color::from_rgba(
                                    theme.cosmic().accent.base.color.red,
                                    theme.cosmic().accent.base.color.green,
                                    theme.cosmic().accent.base.color.blue,
                                    0.2,
                                ),
                                width: 1.0,
                                radius: (20.0 / 8.0).into(),
                            },
                            ..Default::default()
                        }),
                    // System Accent label with description
                    text::body("System accent color").size(11),
                    // Spacer to push content to the left
                    Space::new().width(Length::Fill),
                ]
                .spacing(spacing.space_xs)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([6, 8])
            .style(move |theme| {
                if is_selected {
                    // Selected state - prominent border with accent color + shadow effect
                    container::Style {
                        background: Some(cosmic::iced::Background::Color(Color::from_rgba(
                            theme.cosmic().accent.base.color.red,
                            theme.cosmic().accent.base.color.green,
                            theme.cosmic().accent.base.color.blue,
                            0.15,
                        ))),
                        border: cosmic::iced::Border {
                            color: theme.cosmic().accent_color().into(),
                            width: 3.0,
                            radius: 8.0.into(),
                        },
                        shadow: cosmic::iced::Shadow {
                            color: Color::from_rgba(
                                theme.cosmic().accent.base.color.red,
                                theme.cosmic().accent.base.color.green,
                                theme.cosmic().accent.base.color.blue,
                                0.3,
                            ),
                            offset: cosmic::iced::Vector::new(0.0, 0.0),
                            blur_radius: 6.0,
                        },
                        ..Default::default()
                    }
                } else {
                    // Normal state - subtle border
                    container::Style {
                        background: Some(cosmic::iced::Background::Color(Color::from_rgba(
                            theme.cosmic().accent.base.color.red,
                            theme.cosmic().accent.base.color.green,
                            theme.cosmic().accent.base.color.blue,
                            0.05,
                        ))),
                        border: cosmic::iced::Border {
                            color: Color::from_rgba(
                                theme.cosmic().accent.base.color.red,
                                theme.cosmic().accent.base.color.green,
                                theme.cosmic().accent.base.color.blue,
                                0.2,
                            ),
                            width: 1.0,
                            radius: 8.0.into(),
                        },
                        ..Default::default()
                    }
                }
            }),
        )
        .on_press(Message::SetVisualizationColor(
            VisualizationColor::SystemAccent,
            selected_theme_is_dark,
        ))
        .interaction(cosmic::iced::mouse::Interaction::Pointer),
        "System Accent",
        cosmic::widget::tooltip::Position::Bottom,
    )
    .into()
}

pub fn create_color_button(
    color: Color,
    vis_color: &VisualizationColor,
    message: Message,
    size: f32,
    is_selected: bool,
) -> Element<'static, Message> {
    // Wrap the color button with a tooltip showing the color name
    cosmic::widget::tooltip(
        // Create a colored clickable area using mouse_area to avoid button background
        mouse_area(
            container(text(""))
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .style(move |theme| {
                    if is_selected {
                        // Selected state - prominent border with accent color + shadow effect
                        container::Style {
                            background: Some(cosmic::iced::Background::Color(color)),
                            border: cosmic::iced::Border {
                                color: theme.cosmic().accent_color().into(),
                                width: 3.0,
                                radius: (size / 8.0).into(),
                            },
                            shadow: cosmic::iced::Shadow {
                                color: Color::from_rgba(
                                    theme.cosmic().accent.base.color.red,
                                    theme.cosmic().accent.base.color.green,
                                    theme.cosmic().accent.base.color.blue,
                                    0.4,
                                ),
                                offset: cosmic::iced::Vector::new(0.0, 0.0),
                                blur_radius: 4.0,
                            },
                            ..Default::default()
                        }
                    } else {
                        // Normal state - subtle border
                        container::Style {
                            background: Some(cosmic::iced::Background::Color(color)),
                            border: cosmic::iced::Border {
                                color: Color::from_rgba(
                                    theme.cosmic().bg_divider().red,
                                    theme.cosmic().bg_divider().green,
                                    theme.cosmic().bg_divider().blue,
                                    0.5,
                                ),
                                width: 1.0,
                                radius: (size / 8.0).into(),
                            },
                            ..Default::default()
                        }
                    }
                }),
        )
        .on_press(message)
        .interaction(cosmic::iced::mouse::Interaction::Pointer),
        container(text(vis_color.to_string())).apply(Element::from),
        cosmic::widget::tooltip::Position::Bottom,
    )
    .into()
}
