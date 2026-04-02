// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::widget::{self, settings};
use super_stt_shared::models::write_method::WriteMethod;

use super::common::page_layout;
use crate::ui::messages::Message;

/// Input simulation page: write method selection
pub fn page(write_method: WriteMethod) -> Element<'static, Message> {
    let methods = [
        WriteMethod::Auto,
        WriteMethod::XdgDesktopPortal,
        WriteMethod::Ydotool,
        WriteMethod::WaylandProtocol,
    ];
    let method_names: Vec<String> = methods
        .iter()
        .map(|m| m.pretty_name().to_string())
        .collect();
    let selected_index = methods.iter().position(|m| *m == write_method);

    let sections = settings::view_column(vec![
        settings::section()
            .title("Input Simulation")
            .add(
                settings::item::builder("Write Method")
                    .description("Controls how transcribed text is typed into applications")
                    .control(widget::dropdown(
                        method_names,
                        selected_index,
                        move |index| Message::WriteMethodChanged(methods[index]),
                    )),
            )
            .into(),
    ]);

    page_layout("Input Simulation", sections)
}
