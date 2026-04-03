// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::Length;
use cosmic::iced_widget::row;
use cosmic::widget::{self, settings, text};

use super::common::page_layout;
use crate::ui::messages::Message;

/// Build the control widget for an API key row.
/// Shows input + Save when unconfigured, or "Configured" + Remove when configured.
fn api_key_control(
    has_key: bool,
    key_input: &str,
    on_input: fn(String) -> Message,
    on_save: Message,
    on_remove: Message,
) -> Element<'_, Message> {
    if has_key {
        row![
            text::body("Configured"),
            widget::button::destructive("Remove").on_press(on_remove),
        ]
        .spacing(12)
        .align_y(cosmic::iced::Alignment::Center)
        .into()
    } else {
        let input = widget::text_input("Enter API key...", key_input)
            .on_input(on_input)
            .password()
            .width(Length::Fill);
        let save = widget::button::standard("Save").on_press(on_save);
        row![input, save]
            .spacing(8)
            .width(Length::Fixed(350.0))
            .into()
    }
}

/// Online models page: toggle, provider API keys
#[allow(clippy::fn_params_excessive_bools)]
pub fn page<'a>(
    allow_online: bool,
    has_openai_key: bool,
    openai_key_input: &'a str,
    has_mistral_key: bool,
    mistral_key_input: &'a str,
    has_deepgram_key: bool,
    deepgram_key_input: &'a str,
) -> Element<'a, Message> {
    let mut sections = vec![
        settings::section()
            .title("Online Models")
            .add(
                settings::item::builder("Allow Online Models")
                    .description(
                        "Your audio will be transmitted to third-party online services for transcription. \
                         Only enable this if you are okay with sharing your audio with the providers you configure below.",
                    )
                    .control(
                        widget::toggler(allow_online).on_toggle(Message::AllowOnlineModelsToggled),
                    ),
            )
            .into(),
    ];

    if allow_online {
        sections.push(
            settings::section()
                .title("API Keys")
                .add(settings::item(
                    "OpenAI",
                    api_key_control(
                        has_openai_key,
                        openai_key_input,
                        Message::OpenAIApiKeyChanged,
                        Message::OpenAIApiKeySaved,
                        Message::OpenAIApiKeyRemoved,
                    ),
                ))
                .add(settings::item(
                    "Mistral",
                    api_key_control(
                        has_mistral_key,
                        mistral_key_input,
                        Message::MistralApiKeyChanged,
                        Message::MistralApiKeySaved,
                        Message::MistralApiKeyRemoved,
                    ),
                ))
                .add(settings::item(
                    "Deepgram",
                    api_key_control(
                        has_deepgram_key,
                        deepgram_key_input,
                        Message::DeepgramApiKeyChanged,
                        Message::DeepgramApiKeySaved,
                        Message::DeepgramApiKeyRemoved,
                    ),
                ))
                .into(),
        );
    }

    let content = settings::view_column(sections);
    page_layout("Online Models", content)
}
