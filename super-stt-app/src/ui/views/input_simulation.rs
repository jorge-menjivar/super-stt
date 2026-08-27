// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::widget::row;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, Id, button, settings, text};
use super_stt_shared::models::write_method::WriteMethod;

use super::common::{error_banner, page_layout};
use crate::ui::messages::{Message, WriteMethodMessage};

/// Id of the test field, so the Test button can focus it before the daemon
/// types: `POST /write_method/test` types into whatever window holds keyboard
/// focus, and it needs to be this field rather than the button.
#[must_use]
pub fn test_field_id() -> Id {
    Id::new("write_method_test_field")
}

/// Input simulation page: write method selection + a live typing test
pub fn page<'a>(
    write_method: WriteMethod,
    test_text: &'a str,
    resolved: Option<WriteMethod>,
    countdown: Option<u8>,
    action_error: Option<&'a str>,
) -> Element<'a, Message> {
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

    let mut method_section = settings::section().title("Input Simulation").add(
        settings::item::builder("Write Method")
            .description("Controls how transcribed text is typed into applications")
            .control(widget::dropdown(
                method_names,
                selected_index,
                move |index| Message::WriteMethod(WriteMethodMessage::Changed(methods[index])),
            )),
    );

    // "Auto" names a chain, not a backend, so the configured value alone never
    // says what is really typing. A test is the only thing that resolves it.
    if let Some(resolved) = resolved {
        method_section = method_section.add(
            settings::item::builder("Active backend")
                .description("What the last test actually typed through")
                .control(text::body(resolved.pretty_name())),
        );
    }
    blocks.push(method_section.into());

    blocks.push(test_section(test_text, countdown));

    page_layout("Input Simulation", settings::view_column(blocks))
}

/// Test section: the in-app typing test, plus the delayed one that targets
/// whatever window the user switches to.
fn test_section(test_text: &str, countdown: Option<u8>) -> Element<'_, Message> {
    let test_row = row![
        widget::text_input("Typed text lands here…", test_text)
            .id(test_field_id())
            .on_input(|text| Message::WriteMethod(WriteMethodMessage::TestInput(text)))
            .width(Length::Fill),
        button::suggested("Type test text")
            .on_press(Message::WriteMethod(WriteMethodMessage::Test)),
    ]
    .align_y(Alignment::Center)
    .spacing(10);

    // Mid-countdown the only useful action is calling it off: starting a second
    // test would race the first into whichever window won the focus.
    let delayed_control: Element<'_, Message> = if let Some(secs) = countdown {
        button::destructive(format!("Cancel — typing in {secs}…"))
            .on_press(Message::WriteMethod(WriteMethodMessage::TestCancel))
            .into()
    } else {
        button::standard("Start countdown")
            .on_press(Message::WriteMethod(WriteMethodMessage::TestDelayed))
            .into()
    };

    settings::section()
        .title("Test")
        .add(
            settings::item::builder("Typing test")
                .description(
                    "Types a short string into the field using the same writing method \
                    that will be used by the daemon.",
                )
                .flex_control(test_row),
        )
        .add(
            settings::item::builder("Test another window")
                .description(
                    "Counts down before typing, so you can switch to the app you \
                    actually dictate into.",
                )
                .control(delayed_control),
        )
        .into()
}
