// SPDX-License-Identifier: GPL-3.0-only
//! Common components and utilities shared across views.

use cosmic::iced::{Alignment, Length};
use cosmic::iced_widget::row;
use cosmic::widget::{self, text};
use cosmic::{Apply, Element};

use crate::ui::icons;
use crate::ui::messages::Message;

/// A scope-tagged action-error banner: a destructive warning glyph plus the
/// message. Shared by the pages that render `AppModel::action_error` (the
/// Customization page and the Configure sheet) so a failed save surfaces
/// inline instead of hijacking the UI. Mirrors the Models card's `card_error`.
pub fn error_banner(message: &str) -> Element<'_, Message> {
    row![
        icons::phosphor_destructive(icons::WARNING, 18.0),
        text::body(message.to_string()),
    ]
    .spacing(cosmic::theme::spacing().space_xs)
    .align_y(Alignment::Center)
    .into()
}

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
// reason: explicit 'a makes the input→output lifetime connection visible in this public API; elision would hide that the returned Element borrows from `title`.
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
