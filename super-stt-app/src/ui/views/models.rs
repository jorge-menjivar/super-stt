// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::Length;
use cosmic::iced_widget::{column, row};
use cosmic::widget::{self, button, settings, space::horizontal as horizontal_space, text};
use cosmic::{Apply, Element};
use super_stt_shared::models::protocol::DownloadProgress;
use super_stt_shared::models::provider::{OnlineProvider, Provider};
use super_stt_shared::models::registry::{self, SourceKind};

use super::common::{go_next_with_item, page_layout};
use crate::core::app::ModelOperationState;
use crate::state::ContextPage;
use crate::ui::icons;
use crate::ui::messages::Message;

/// Custom models directory section
fn custom_models_section<'a>(
    custom_models_dir: Option<&'a str>,
    custom_models_dir_input: &'a str,
    editing: bool,
) -> Element<'a, Message> {
    let display_value = if editing {
        custom_models_dir_input
    } else {
        custom_models_dir.unwrap_or("")
    };

    let mut input = widget::text_input("Path to custom models directory...", display_value);
    if editing {
        input = input.on_input(Message::CustomModelsDirInput);
    }

    let mut buttons: Vec<Element<'_, Message>> = Vec::new();

    if editing {
        buttons.push(
            button::standard("Apply")
                .on_press(Message::CustomModelsDirSet(
                    if custom_models_dir_input.trim().is_empty() {
                        None
                    } else {
                        Some(custom_models_dir_input.trim().to_string())
                    },
                ))
                .into(),
        );
        buttons.push(
            button::text("Cancel")
                .on_press(Message::CustomModelsDirEdit(false))
                .into(),
        );
    } else {
        buttons.push(
            button::standard("Edit")
                .on_press(Message::CustomModelsDirEdit(true))
                .into(),
        );
        if custom_models_dir.is_some() {
            buttons.push(
                button::destructive("Reset")
                    .on_press(Message::CustomModelsDirSet(None))
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
        .title("Custom Models Location")
        .add(
            settings::item::builder("Custom Models Directory")
                .description(
                    "Add custom or fine-tuned models by placing them in this directory. \
                     Each subdirectory with a config.json is detected as a model \
                     (e.g. hf download my-org/whisper-medical --local-dir ./custom-models/whisper-medical).",
                )
                .flex_control(controls),
        )
        .into()
}

/// Model section when ready: shows model picker (context drawer)
fn model_ready_section(current_model: &str) -> Element<'_, Message> {
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
fn model_downloading_section<'a>(
    target_model: &'a String,
    progress: &'a DownloadProgress,
) -> Element<'a, Message> {
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
fn model_loading_section<'a>(
    target_model: &'a String,
    status_message: &'a str,
) -> Element<'a, Message> {
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

/// Model section when an error occurred
fn model_error_section(error_message: &str) -> Element<'_, Message> {
    let error_widget = column![
        row![
            icons::phosphor(icons::WARNING).size(20),
            text::body(error_message),
        ]
        .spacing(cosmic::theme::spacing().space_xs)
        .align_y(cosmic::iced::Alignment::Center),
        widget::button::standard("Dismiss").on_press(Message::ModelChanged {
            model: registry::default_definition().name.to_string(),
            provider: registry::default_definition().provider,
            source: registry::default_definition().source.kind(),
        }),
    ]
    .spacing(cosmic::theme::spacing().space_s);

    settings::section()
        .title("Speech-to-Text Model")
        .add(settings::flex_item("Error", error_widget))
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

/// Format bytes as a human-readable size string (always in GB)
#[allow(clippy::cast_precision_loss)]
fn format_size(bytes: u64) -> String {
    const GB: f64 = 1_073_741_824.0;
    if bytes == 0 {
        return String::new();
    }
    let gb = bytes as f64 / GB;
    if gb >= 10.0 {
        format!("{gb:.0} GB")
    } else {
        format!("{gb:.1} GB")
    }
}

/// Build a model row for the selection list.
/// `effective_free_vram` is the GPU memory that would be available after unloading
/// the current model (None if GPU is off or unknown).
fn model_row(
    model: &str,
    provider: Provider,
    source: SourceKind,
    current_model: &str,
    current_provider: Provider,
    current_source: SourceKind,
    effective_free_vram: Option<u64>,
) -> Element<'static, Message> {
    let selected =
        model == current_model && provider == current_provider && source == current_source;

    let svg_accent = |theme: &cosmic::theme::Theme| {
        let accent = theme.cosmic().accent_color();
        cosmic::widget::svg::Style {
            color: Some(cosmic::iced::Color::from(accent)),
        }
    };

    // VRAM info is only available for standard models
    let vram_required = registry::find_by(model, provider).map_or(0, |d| d.estimated_vram_bytes);
    let size_label = format_size(vram_required);
    let wont_fit =
        vram_required > 0 && effective_free_vram.is_some_and(|free| vram_required > free);

    let mut items: Vec<Element<'static, Message>> = vec![
        text::body(model.to_string())
            .class(if selected {
                cosmic::theme::Text::Accent
            } else {
                cosmic::theme::Text::Default
            })
            .width(Length::Fill)
            .into(),
    ];

    if wont_fit {
        items.push(icons::phosphor(icons::WARNING).size(16).into());
    }

    if !size_label.is_empty() {
        items.push(text::caption(size_label).into());
    }

    items.push(if selected {
        icons::phosphor(icons::CHECK)
            .size(16)
            .class(cosmic::theme::Svg::Custom(std::rc::Rc::new(Box::new(
                svg_accent,
            ))))
            .into()
    } else {
        horizontal_space().width(16.).into()
    });

    let model_owned = model.to_string();
    settings::item_row(items)
        .apply(widget::container)
        .width(Length::Fill)
        .class(cosmic::theme::Container::List)
        .apply(widget::button::custom)
        .width(Length::Fill)
        .class(cosmic::theme::Button::Transparent)
        .on_press(Message::ModelSelected {
            model: model_owned,
            provider,
            source,
        })
        .into()
}

/// Build the model selection list for the context drawer (font-picker pattern).
/// `gpu_memory` is `Some((free, total))` when GPU is enabled, used to warn about models that won't fit.
pub fn model_selection_list(
    available_models: &[(String, Provider, SourceKind)],
    current_model: &str,
    current_provider: Provider,
    current_source: SourceKind,
    search: &str,
    gpu_enabled: bool,
    gpu_memory: super_stt_shared::daemon::http_client::GpuMemoryInfo,
) -> Element<'static, Message> {
    // Effective free VRAM = current free + what the current model uses (it gets unloaded on switch)
    let current_vram =
        registry::find_by(current_model, current_provider).map_or(0, |d| d.estimated_vram_bytes);
    let effective_free_vram = if gpu_enabled {
        gpu_memory.map(|(free, _total)| free + current_vram)
    } else {
        None
    };

    let search_lower = search.to_lowercase();
    let models: Vec<&(String, Provider, SourceKind)> = if search.is_empty() {
        available_models.iter().collect()
    } else {
        available_models
            .iter()
            .filter(|(name, _, _)| name.to_lowercase().contains(&search_lower))
            .collect()
    };

    // Partition by (provider, source). Customs go to their own section
    // regardless of underlying engine.
    let mut local: Vec<&(String, Provider, SourceKind)> = Vec::new();
    let mut openai: Vec<&(String, Provider, SourceKind)> = Vec::new();
    let mut mistral: Vec<&(String, Provider, SourceKind)> = Vec::new();
    let mut deepgram: Vec<&(String, Provider, SourceKind)> = Vec::new();
    let mut custom: Vec<&(String, Provider, SourceKind)> = Vec::new();

    for entry in &models {
        if matches!(entry.2, SourceKind::Custom) {
            custom.push(entry);
            continue;
        }
        match entry.1 {
            Provider::LocalWhisper | Provider::LocalVoxtral => local.push(entry),
            Provider::Online(OnlineProvider::OpenAI) => openai.push(entry),
            Provider::Online(OnlineProvider::Mistral) => mistral.push(entry),
            Provider::Online(OnlineProvider::Deepgram) => deepgram.push(entry),
        }
    }

    let render_section = |title: &str, entries: Vec<&(String, Provider, SourceKind)>| {
        let list =
            entries
                .into_iter()
                .fold(widget::list_column(), |list, (name, provider, source)| {
                    list.add(model_row(
                        name,
                        *provider,
                        *source,
                        current_model,
                        current_provider,
                        current_source,
                        effective_free_vram,
                    ))
                });
        column![text::title4(title.to_string()), list].spacing(cosmic::theme::spacing().space_xxs)
    };

    let mut sections: Vec<Element<'static, Message>> = Vec::new();
    if !custom.is_empty() {
        sections.push(render_section("Custom", custom).into());
    }
    if !local.is_empty() {
        sections.push(render_section("Local", local).into());
    }
    if !openai.is_empty() {
        sections.push(render_section("OpenAI (online)", openai).into());
    }
    if !mistral.is_empty() {
        sections.push(render_section("Mistral (online)", mistral).into());
    }
    if !deepgram.is_empty() {
        sections.push(render_section("Deepgram (online)", deepgram).into());
    }

    column(sections)
        .spacing(cosmic::theme::spacing().space_m)
        .width(Length::Fill)
        .into()
}

/// Context drawer header: device toggle + search input.
///
/// While a device switch is in flight (`device_state == Switching` or
/// `Cooldown`), the toggle is **disabled** — its `on_toggle` handler
/// is dropped so clicks are ignored. The displayed state reflects the
/// target device of the in-flight switch (so the toggle visually
/// flips immediately on click) and the toggle is dimmed to ~50%
/// opacity to make the disabled state visually obvious. On switch
/// completion (or error) the toggle re-enables and snaps to whatever
/// `current_device` ended up being.
pub fn model_drawer_header<'a>(
    search: &'a str,
    current_device: &'a str,
    available_devices: &'a [String],
    device_state: &'a crate::core::app::DeviceState,
    gpu_memory: super_stt_shared::daemon::http_client::GpuMemoryInfo,
) -> Element<'a, Message> {
    use crate::core::app::DeviceState;

    let has_gpu = available_devices.contains(&"cuda".to_string());

    let search_input: Element<'a, Message> = widget::search_input("Search models...", search)
        .on_input(Message::ModelSearchChanged)
        .on_clear(Message::ModelSearchChanged(String::new()))
        .into();

    if !has_gpu {
        return search_input;
    }

    // Effective device shown by the toggle: while switching, show the
    // target so the toggle visually flips the moment the user clicks
    // it; otherwise show the live `current_device`.
    let device_switching = matches!(device_state, DeviceState::Switching { .. });
    let effective_device = match device_state {
        DeviceState::Switching { target_device, .. } => target_device.as_str(),
        _ => current_device,
    };
    let gpu_enabled = effective_device == "cuda";
    // Cooldown is the brief window after the daemon's "ready" event
    // before we accept a new switch — UI stays disabled to debounce
    // rapid toggles, but the toggle's displayed state is already
    // `current_device`.
    let toggle_disabled = device_switching || matches!(device_state, DeviceState::Cooldown);

    // The cosmic toggler renders itself in a "disabled" appearance
    // when its `on_toggle` handler is `None` — we drop the handler
    // while a switch is in flight (or in the brief cooldown that
    // follows) to block re-entry until the daemon confirms the
    // result.
    let mut toggler = cosmic::widget::toggler(gpu_enabled);
    if !toggle_disabled {
        toggler = toggler.on_toggle(move |on| {
            Message::DeviceSelected(if on { "cuda" } else { "cpu" }.to_string())
        });
    }

    let mut items = column![].spacing(cosmic::theme::spacing().space_s);

    let description = if device_switching {
        "Switching device — please wait..."
    } else if matches!(device_state, DeviceState::Cooldown) {
        "Finishing previous switch..."
    } else {
        "Enable to use more powerful models and faster transcriptions"
    };

    items = items.push(
        settings::item::builder("GPU Acceleration")
            .description(description)
            .control(toggler),
    );

    if let Some((free, total)) = gpu_memory.filter(|_| gpu_enabled) {
        let used = total.saturating_sub(free);
        let mem_text = format!(
            "GPU Memory: {} / {} used",
            format_size(used),
            format_size(total)
        );
        items = items.push(text::caption(mem_text));
    }

    items = items.push(search_input);

    items.into()
}

/// Models page view
pub fn page<'a>(
    current_model: &'a str,
    model_operation_state: &'a ModelOperationState,
    device_state: &'a crate::core::app::DeviceState,
    custom_models_dir: Option<&'a str>,
    custom_models_dir_input: &'a str,
    custom_models_dir_editing: bool,
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
        } => model_downloading_section(target_model, progress),
        ModelOperationState::Loading {
            target_model,
            status_message,
        } => model_loading_section(target_model, status_message),
        ModelOperationState::Error { message } => model_error_section(message),
    };

    let storage_section = custom_models_section(
        custom_models_dir,
        custom_models_dir_input,
        custom_models_dir_editing,
    );

    page_layout(
        "Models",
        settings::view_column(vec![model_section, storage_section]),
    )
}
