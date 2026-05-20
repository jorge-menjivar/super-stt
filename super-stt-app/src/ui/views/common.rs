// SPDX-License-Identifier: GPL-3.0-only
//! Common components and utilities shared across views.

use cosmic::iced::Length;
use cosmic::iced_core::Alignment;
use cosmic::iced_core::text::Wrapping;
use cosmic::widget::{self, button, settings, space::horizontal as horizontal_space, text};
use cosmic::{Apply, Element};

use crate::ui::icons;
use crate::ui::messages::Message;

/// Create a page container following cosmic-settings patterns
pub fn page_container<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    let theme = cosmic::theme::active();
    let padding = theme.cosmic().space_l();
    let bottom_spacer = theme.cosmic().space_m();

    widget::container(content.into())
        .max_width(800)
        .width(Length::Fill)
        .apply(widget::container)
        .center_x(Length::Fill)
        .padding([0, padding, bottom_spacer, padding])
        .into()
}

/// Create page title header
#[allow(clippy::elidable_lifetime_names)]
pub fn page_header<'a>(title: &'a str) -> Element<'a, Message> {
    page_container(text::title3(title))
}

/// Create scrollable page content
pub fn page_content<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    page_container(content.into())
        .apply(widget::scrollable)
        .height(Length::Fill)
        .into()
}

/// Create a standard two-part page layout (header + scrollable content)
pub fn page_layout<'a>(
    title: &'a str,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    widget::column::with_capacity(2)
        .push(page_header(title))
        .push(page_content(content))
        .height(Length::Fill)
        .into()
}

/// A clickable settings item that shows a description, current value, and a "go next" arrow.
/// Used for items that open a context drawer (like font/model selection in cosmic-settings).
pub fn go_next_with_item<'a>(
    description: &'a str,
    item: impl Into<Element<'a, Message>>,
    msg: Message,
) -> Element<'a, Message> {
    settings::item_row(vec![
        text::body(description).wrapping(Wrapping::Word).into(),
        horizontal_space().into(),
        widget::row::with_capacity(2)
            .push(item)
            .push(icons::phosphor(icons::CARET_RIGHT).size(16))
            .align_y(Alignment::Center)
            .spacing(cosmic::theme::spacing().space_s)
            .into(),
    ])
    .width(Length::Fill)
    .apply(widget::container)
    .class(cosmic::theme::Container::List)
    .width(Length::Fill)
    .apply(button::custom)
    .padding(0)
    .width(Length::Fill)
    .class(cosmic::theme::Button::Transparent)
    .on_press(msg)
    .into()
}
