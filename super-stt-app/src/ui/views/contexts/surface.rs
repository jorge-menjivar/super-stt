// SPDX-License-Identifier: GPL-3.0-only
//! The two container styles this page draws with.
//!
//! Deliberately its own small module rather than a reach into
//! [`views::models::surface`](crate::ui::views::models::surface): those helpers
//! are `pub(super)` to the models tree, and widening a dozen of them so one
//! other page can borrow two would make every one of them a shared surface that
//! has to be kept stable. The card here is the same shape by construction — it
//! reads the same theme tokens — and that is the part worth sharing.

use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget;

use crate::ui::messages::Message;

/// A card: the page's one raised surface, accented when it is the active
/// context.
pub(super) fn card<'a>(
    content: impl Into<Element<'a, Message>>,
    active: bool,
) -> Element<'a, Message> {
    widget::container(content.into())
        .padding(cosmic::theme::spacing().space_s)
        .width(Length::Fill)
        .class(cosmic::theme::Container::custom(move |theme| {
            let cosmic = theme.cosmic();
            let component = &theme.current_container().component;
            let border_color = if active {
                cosmic.accent_color().into()
            } else {
                component.divider.into()
            };
            cosmic::iced::widget::container::Style {
                icon_color: Some(component.on.into()),
                text_color: Some(component.on.into()),
                background: Some(cosmic::iced::Background::Color(component.base.into())),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_m.into(),
                    width: 1.0,
                    color: border_color,
                },
                shadow: cosmic::iced::Shadow {
                    color: cosmic::iced::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.12,
                    },
                    offset: cosmic::iced::Vector::new(0.0, 1.0),
                    blur_radius: 4.0,
                },
                snap: true,
            }
        }))
        .into()
}

/// A faint rule inside a card.
pub(super) fn plain_divider<'a>() -> Element<'a, Message> {
    widget::divider::horizontal::default().into()
}

/// Dim a secondary line so it reads as a caption rather than as body text.
pub(super) fn muted<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    widget::container(content.into())
        .class(cosmic::theme::Container::custom(|theme| {
            let mut color: cosmic::iced::Color = theme.current_container().component.on.into();
            color.a *= 0.7;
            cosmic::iced::widget::container::Style {
                text_color: Some(color),
                ..Default::default()
            }
        }))
        .into()
}
