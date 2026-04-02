// SPDX-License-Identifier: GPL-3.0-only
use crate::app::Message;
use cosmic::{
    Renderer, Theme,
    applet::menu_button,
    iced::{
        Length,
        widget::{self, column},
    },
    widget::text,
};

pub fn revealer_head(
    _open: bool,
    title: String,
    selected: String,
    toggle: Message,
) -> cosmic::widget::Button<'static, Message> {
    menu_button(column![
        text::body(title).width(Length::Fill),
        text::caption(selected),
    ])
    .on_press(toggle)
}

pub fn revealer(
    open: bool,
    title: String,
    selected: String,
    options: &[(String, String)],
    toggle: Message,
    mut change: impl FnMut(String) -> Message + 'static,
) -> widget::Column<'static, Message, Theme, Renderer> {
    if open {
        options.iter().fold(
            column![revealer_head(open, title, selected, toggle)].width(Length::Fill),
            |col, (id, name)| {
                col.push(
                    menu_button(text::body(name.clone()))
                        .on_press(change(id.clone()))
                        .width(Length::Fill)
                        .padding([8, 48]),
                )
            },
        )
    } else {
        column![revealer_head(open, title, selected, toggle)]
    }
}
