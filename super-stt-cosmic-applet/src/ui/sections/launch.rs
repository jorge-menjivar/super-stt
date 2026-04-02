// SPDX-License-Identifier: GPL-3.0-only
use crate::app::Message;
use cosmic::{
    Element,
    applet::menu_button,
    iced::{Length, widget::column},
    widget::text,
};

pub fn create_launch_section() -> Element<'static, Message> {
    column![
        menu_button(text::body("Launch Super STT App"))
            .on_press(Message::LaunchApp)
            .width(Length::Fill)
    ]
    .spacing(4)
    .into()
}
