// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::widget::{self, settings};
use super_stt_shared::models::write_method::WriteMethod;

use super::common::{error_banner, page_layout};
use crate::ui::messages::{Message, WriteMethodMessage};

/// Input simulation page: write method selection
pub fn page(write_method: WriteMethod, action_error: Option<&str>) -> Element<'_, Message> {
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

    let mut blocks = Vec::new();
    if let Some(message) = action_error {
        blocks.push(error_banner(message));
    }
    blocks.push(
        settings::section()
            .title("Input Simulation")
            .add(
                settings::item::builder("Write Method")
                    .description("Controls how transcribed text is typed into applications")
                    .control(widget::dropdown(
                        method_names,
                        selected_index,
                        move |index| {
                            Message::WriteMethod(WriteMethodMessage::Changed(methods[index]))
                        },
                    )),
            )
            .into(),
    );

    page_layout("Input Simulation", settings::view_column(blocks))
}
