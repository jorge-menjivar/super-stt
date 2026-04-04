// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::Length;
use cosmic::iced_widget::{column, row};
use cosmic::widget::{self, button, settings, space::horizontal as horizontal_space, text};
use cosmic::{Apply, Element};
use super_stt_shared::models::protocol::DownloadProgress;
use super_stt_shared::stt_model::STTModel;

use super::common::{go_next_with_item, page_layout};
use crate::core::app::ModelOperationState;
use crate::state::ContextPage;
use crate::ui::messages::Message;

/// Model storage section: model override path
fn model_storage_section<'a>(
    model_override_path: Option<&'a str>,
    model_override_path_input: &'a str,
    editing: bool,
) -> Element<'a, Message> {
    let display_value = if editing {
        model_override_path_input
    } else {
        model_override_path.unwrap_or("")
    };

    let mut input = widget::text_input("Path to model files...", display_value);
    if editing {
        input = input.on_input(Message::ModelOverridePathInput);
    }

    let mut buttons: Vec<Element<'_, Message>> = Vec::new();

    if editing {
        buttons.push(
            button::standard("Apply")
                .on_press(Message::ModelOverridePathSet(
                    if model_override_path_input.trim().is_empty() {
                        None
                    } else {
                        Some(model_override_path_input.trim().to_string())
                    },
                ))
                .into(),
        );
        buttons.push(
            button::text("Cancel")
                .on_press(Message::ModelOverridePathEdit(false))
                .into(),
        );
    } else {
        buttons.push(
            button::standard("Edit")
                .on_press(Message::ModelOverridePathEdit(true))
                .into(),
        );
        if model_override_path.is_some() {
            buttons.push(
                button::destructive("Reset")
                    .on_press(Message::ModelOverridePathSet(None))
                    .into(),
            );
        }
    }

    let controls = row![
        widget::container(input).width(Length::Fill),
        row(buttons).spacing(8),
    ]
    .spacing(8);

    settings::section()
        .title("Model Storage")
        .add(
            settings::item::builder("Model Override Path")
                .description(
                    "Models here override the default cache. \
                     Copy a HuggingFace model dir renamed to its model name \
                     (e.g. whisper-small/snapshots/main/).",
                )
                .flex_control(controls),
        )
        .into()
}

/// Model section when ready: shows model picker (context drawer)
fn model_ready_section(current_model: &STTModel) -> Element<'_, Message> {
    settings::section()
        .title("Speech-to-Text Model")
        .add(go_next_with_item(
            "Model",
            text::body(current_model.to_string()),
            Message::ToggleContextPage(ContextPage::ModelSelection),
        ))
        .into()
}

/// Model section when downloading
#[allow(clippy::cast_precision_loss)]
fn model_downloading_section(
    target_model: STTModel,
    progress: &DownloadProgress,
) -> Element<'_, Message> {
    let progress_fraction = if progress.percentage < 0.0 || progress.percentage > 100.0 {
        log::warn!(
            "Invalid progress percentage: {} (must be 0-100)",
            progress.percentage
        );
        0.0
    } else {
        (progress.percentage / 100.0).clamp(0.0, 1.0)
    };

    let progress_text = format!(
        "Downloading {} ({}/{}): {:.1}%",
        target_model,
        progress.file_index + 1,
        progress.total_files,
        progress.percentage
    );

    let eta_text = if let Some(eta_seconds) = progress.eta_seconds {
        if eta_seconds > 0 {
            let minutes = eta_seconds / 60;
            let seconds = eta_seconds % 60;
            if minutes > 0 {
                format!("ETA: {minutes}m {seconds}s")
            } else {
                format!("ETA: {seconds}s")
            }
        } else {
            "Finishing...".to_string()
        }
    } else {
        String::new()
    };

    let bytes_text = if progress.total_bytes > 0 {
        let mb_downloaded = progress.bytes_downloaded as f64 / (1024.0 * 1024.0);
        let mb_total = progress.total_bytes as f64 / (1024.0 * 1024.0);
        format!("{mb_downloaded:.1} / {mb_total:.1} MB")
    } else {
        String::new()
    };

    let details_widget = column![
        text::body(progress_text),
        widget::progress_bar(0.0..=1.0, progress_fraction.max(0.1)),
        row![
            text::body(bytes_text).width(Length::Fill),
            text::body(eta_text).width(Length::Fill),
        ]
        .spacing(10),
    ]
    .spacing(10);

    settings::section()
        .title("Speech-to-Text Model")
        .add(settings::flex_item("Status", details_widget))
        .add(settings::item(
            "Cancel",
            button::destructive("Cancel Download").on_press(Message::CancelDownload),
        ))
        .into()
}

/// Model section when loading
fn model_loading_section(target_model: STTModel, status_message: &str) -> Element<'_, Message> {
    let status_text = format!("Loading {target_model}: {status_message}");

    let details_widget = column![
        text::body(status_text),
        widget::progress_bar(0.0..=1.0, 0.5),
    ]
    .spacing(10);

    settings::section()
        .title("Speech-to-Text Model")
        .add(settings::flex_item("Status", details_widget))
        .into()
}

/// Model section when device is switching
fn model_device_switching_section<'a>(
    target_device: &'a str,
    status_message: &'a str,
) -> Element<'a, Message> {
    let device_display = if target_device == "cpu" {
        "CPU"
    } else if target_device == "cuda" {
        "CUDA GPU"
    } else {
        target_device
    };

    let status_text = format!("Switching to {device_display}: {status_message}");

    let details_widget = column![
        text::body(status_text),
        widget::progress_bar(0.0..=1.0, 0.5),
    ]
    .spacing(10);

    settings::section()
        .title("Speech-to-Text Model")
        .add(settings::flex_item("Status", details_widget))
        .into()
}

/// Build a model row for the selection list
fn model_row(model: STTModel, current_model: STTModel) -> Element<'static, Message> {
    let selected = model == current_model;

    let svg_accent = |theme: &cosmic::theme::Theme| {
        let accent = theme.cosmic().accent_color();
        cosmic::widget::svg::Style {
            color: Some(cosmic::iced::Color::from(accent)),
        }
    };

    settings::item_row(vec![
        text::body(model.to_string())
            .class(if selected {
                cosmic::theme::Text::Accent
            } else {
                cosmic::theme::Text::Default
            })
            .width(Length::Fill)
            .into(),
        if selected {
            cosmic::widget::icon::from_name("object-select-symbolic")
                .size(16)
                .icon()
                .class(cosmic::theme::Svg::Custom(std::rc::Rc::new(Box::new(
                    svg_accent,
                ))))
                .into()
        } else {
            horizontal_space().width(16.).into()
        },
    ])
    .apply(widget::container)
    .class(cosmic::theme::Container::List)
    .apply(widget::button::custom)
    .class(cosmic::theme::Button::Transparent)
    .on_press(Message::ModelSelected(model))
    .into()
}

/// Build the model selection list for the context drawer (font-picker pattern)
pub fn model_selection_list(
    available_models: Vec<STTModel>,
    current_model: STTModel,
    search: &str,
) -> Element<'static, Message> {
    let search_lower = search.to_lowercase();
    let models: Vec<STTModel> = if search.is_empty() {
        available_models
    } else {
        available_models
            .into_iter()
            .filter(|m| m.to_string().to_lowercase().contains(&search_lower))
            .collect()
    };

    let local: Vec<STTModel> = models.iter().filter(|m| !m.is_online()).copied().collect();

    // Group online models by provider
    let openai: Vec<STTModel> = models
        .iter()
        .filter(|m| m.is_online() && m.api_provider() == "openai")
        .copied()
        .collect();
    let mistral: Vec<STTModel> = models
        .iter()
        .filter(|m| m.is_online() && m.api_provider() == "mistral")
        .copied()
        .collect();
    let deepgram: Vec<STTModel> = models
        .iter()
        .filter(|m| m.is_online() && m.api_provider() == "deepgram")
        .copied()
        .collect();

    let mut sections: Vec<Element<'static, Message>> = Vec::new();

    if !local.is_empty() {
        let list = local
            .into_iter()
            .fold(widget::list_column(), |list, model| {
                list.add(model_row(model, current_model))
            });
        sections.push(
            column![text::title4("Local"), list]
                .spacing(cosmic::theme::spacing().space_xxs)
                .into(),
        );
    }

    if !openai.is_empty() {
        let list = openai
            .into_iter()
            .fold(widget::list_column(), |list, model| {
                list.add(model_row(model, current_model))
            });
        sections.push(
            column![text::title4("OpenAI (online)"), list]
                .spacing(cosmic::theme::spacing().space_xxs)
                .into(),
        );
    }

    if !mistral.is_empty() {
        let list = mistral
            .into_iter()
            .fold(widget::list_column(), |list, model| {
                list.add(model_row(model, current_model))
            });
        sections.push(
            column![text::title4("Mistral (online)"), list]
                .spacing(cosmic::theme::spacing().space_xxs)
                .into(),
        );
    }

    if !deepgram.is_empty() {
        let list = deepgram
            .into_iter()
            .fold(widget::list_column(), |list, model| {
                list.add(model_row(model, current_model))
            });
        sections.push(
            column![text::title4("Deepgram (online)"), list]
                .spacing(cosmic::theme::spacing().space_xxs)
                .into(),
        );
    }

    column(sections)
        .spacing(cosmic::theme::spacing().space_m)
        .into()
}

/// Context drawer header: device toggle + search input
pub fn model_drawer_header<'a>(
    search: &'a str,
    current_device: &'a str,
    available_devices: &'a [String],
    device_switching: bool,
) -> Element<'a, Message> {
    let has_gpu = available_devices.contains(&"cuda".to_string());
    let gpu_enabled = current_device == "cuda";

    let search_input: Element<'a, Message> = widget::search_input("Search models...", search)
        .on_input(Message::ModelSearchChanged)
        .on_clear(Message::ModelSearchChanged(String::new()))
        .into();

    if !has_gpu {
        return search_input;
    }

    let mut toggler = cosmic::widget::toggler(gpu_enabled);
    if !device_switching {
        toggler = toggler.on_toggle(move |on| {
            Message::DeviceSelected(if on { "cuda" } else { "cpu" }.to_string())
        });
    }

    column![
        settings::item::builder("GPU Acceleration")
            .description("Use CUDA GPU for faster transcription")
            .control(toggler),
        search_input,
    ]
    .spacing(cosmic::theme::spacing().space_s)
    .into()
}

/// Models page view
pub fn page<'a>(
    current_model: &'a STTModel,
    model_operation_state: &'a ModelOperationState,
    device_state: &'a crate::core::app::DeviceState,
    model_override_path: Option<&'a str>,
    model_override_path_input: &'a str,
    model_override_path_editing: bool,
) -> Element<'a, Message> {
    let model_section = match model_operation_state {
        ModelOperationState::Ready => {
            if let crate::core::app::DeviceState::Switching {
                target_device,
                status_message,
            } = device_state
            {
                model_device_switching_section(target_device, status_message)
            } else {
                model_ready_section(current_model)
            }
        }
        ModelOperationState::Downloading {
            target_model,
            progress,
        } => model_downloading_section(*target_model, progress),
        ModelOperationState::Loading {
            target_model,
            status_message,
        } => model_loading_section(*target_model, status_message),
    };

    let storage_section = model_storage_section(
        model_override_path,
        model_override_path_input,
        model_override_path_editing,
    );

    page_layout(
        "Models",
        settings::view_column(vec![model_section, storage_section]),
    )
}
