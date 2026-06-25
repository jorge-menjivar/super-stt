// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::iced_widget::{column, row};
use cosmic::widget::{self, button, text};
use super_stt_shared::models::protocol::DownloadProgress;

use crate::core::app::{AppModel, ModelOperationState};
use crate::daemon::backends::BackendInfo;
use crate::ui::icons;
use crate::ui::messages::Message;

use super::chips::{
    backend_is_online, backend_supports_cpu, backend_supports_gpu, capability_chips, count_chip,
    requirement_warning,
};
use super::fmt::vram_warning;
use super::status::unmet_requirements;
use super::surface::{card_divider, card_surface};

/// The leading glyph tile shared by every backend card: a soft accent-tinted
/// rounded square holding the "models" brain glyph. Gives each card a
/// consistent anchor on the left that lines up with the two-line name/source
/// block beside it.
pub(super) fn backend_glyph_tile<'a>() -> Element<'a, Message> {
    let accent: cosmic::iced::Color = cosmic::theme::active().cosmic().accent.base.into();
    let mut fill = accent;
    fill.a = 0.16;
    let size = 40.0_f32;

    widget::container(icons::phosphor_tinted(icons::BRAIN, 22.0, accent))
        .center_x(Length::Fixed(size))
        .center_y(Length::Fixed(size))
        .class(cosmic::theme::Container::custom(move |theme| {
            cosmic::iced_widget::container::Style {
                background: Some(cosmic::iced::Background::Color(fill)),
                border: cosmic::iced::Border {
                    radius: theme.cosmic().corner_radii.radius_s.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .into()
}

/// Assemble a backend card's header: the leading glyph tile, a two-line name +
/// source block that takes the remaining width, and the card's action buttons
/// grouped on the right.
pub(super) fn backend_header(
    name: String,
    source: String,
    actions: Vec<Element<'_, Message>>,
) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let muted = super::surface::muted_text_color();
    let title_block = column![
        // title4's default line box is Absolute(30) around 20px text, so ~5px of
        // leading sits above the glyph and reads as extra padding at the top of
        // the card. Hug the glyph with line-height 1.0 — same fix as the chips.
        text::title4(name).line_height(1.0),
        text::caption(source).class(cosmic::theme::Text::Color(muted)),
    ]
    .spacing(spacing.space_xxxs)
    .width(Length::Fill);
    let actions_row = row(actions)
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center);
    row![backend_glyph_tile(), title_block, actions_row]
        .spacing(spacing.space_s)
        .align_y(Alignment::Center)
        .into()
}

/// Language row for the active-backend card: shown only when `model_loaded` and
/// the daemon's resolution block reports the model as multilingual. Returns `None`
/// otherwise so the caller can skip `card.push(…)` entirely.
fn language_row<'a>(
    model_loaded: bool,
    app: &'a AppModel,
    spacing: &cosmic::cosmic_theme::Spacing,
) -> Option<Element<'a, Message>> {
    let block = app.active_model_language.as_ref()?;
    if !model_loaded
        || block
            .get("multilingual")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    {
        return None;
    }
    let effective = block.get("effective").and_then(serde_json::Value::as_str);
    let source_str = block
        .get("source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("default");
    let primary = block
        .get("primary")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let label = match (effective, source_str) {
        (Some(tag), "override") => crate::ui::languages::friendly_name(tag),
        (Some(tag), "global") => {
            format!("{} · global", crate::ui::languages::friendly_name(tag))
        }
        _ => format!("{} · default", crate::ui::languages::friendly_name(primary)),
    };
    Some(
        widget::row::with_capacity(2)
            .spacing(spacing.space_s)
            .align_y(Alignment::Center)
            .push(text::body("Language").width(Length::Fill))
            .push(
                widget::button::standard(label)
                    .on_press(Message::OpenLanguagePicker { per_model: true }),
            )
            .into(),
    )
}

/// The selected backend, shown above the tabs: its model picker + Select, an
/// in-card status line (loading / download progress / error), Configure, and
/// Deselect.
pub(super) fn active_backend_card<'a>(
    backend: &'a BackendInfo,
    app: &'a AppModel,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let online = backend_is_online(backend);
    let source = backend.source.clone();
    let model_loaded = !app.current_source.is_empty() && app.current_source == backend.source;
    let missing = unmet_requirements(&app.backend_secret_configured, backend);

    let actions: Vec<Element<'a, Message>> = vec![
        button::standard("Configure")
            .on_press(Message::OpenBackendConfig(source.clone()))
            .into(),
        button::destructive("Deselect")
            .on_press(Message::DeselectBackend)
            .into(),
    ];

    let mut card = widget::column::with_capacity(6)
        .spacing(spacing.space_s)
        .push(backend_header(
            backend.name.clone(),
            backend.source.clone(),
            actions,
        ));

    // Capability chips advertise the backend's compute: GPU / CPU for local
    // models, Cloud for online ones (with the hosts it reaches on hover); a
    // trailing "N models" count chip rounds out the row.
    let hosts = online.then(|| {
        crate::daemon::catalog::by_source(&backend.source)
            .map_or(&[][..], |c| c.allowed_hosts.as_slice())
    });
    let mut chip_row = row![].spacing(spacing.space_xxs).align_y(Alignment::Center);
    if let Some(chips) = capability_chips(
        backend_supports_gpu(backend),
        backend_supports_cpu(backend),
        hosts,
    ) {
        chip_row = chip_row.push(chips);
    }
    let model_count = backend.models.len();
    if model_count > 0 {
        let label = if model_count == 1 {
            "1 model".to_string()
        } else {
            format!("{model_count} models")
        };
        chip_row = chip_row.push(count_chip(label));
    }
    card = card.push(chip_row);

    // Loaded vs idle. When a model is loaded for this backend, show a summary
    // and an Unload button; otherwise show the model + device staging row
    // with a Load button. Requirements-unmet skips both — the warnings below
    // are the only path forward. A divider sets the launch controls apart from
    // the header/chips above.
    if missing.is_empty() {
        card = card.push(card_divider());
        if model_loaded {
            card = card.push(loaded_model_summary(app));
        } else {
            card = card.push(staged_model_picker(backend, app));
        }
    }

    // Per-model language trigger — only when this backend's model is active and
    // the daemon reports it multilingual.
    if let Some(row) = language_row(model_loaded, app, &spacing) {
        card = card.push(row);
    }

    // Unmet requirements are surfaced inline so the user fixes them in this
    // same card (Configure) without ever triggering the daemon's safety-net
    // error. Card-scoped: no need to repeat the backend name in each line.
    for label in &missing {
        card = card.push(requirement_warning(label));
    }

    // The model operation status for this backend, shown inside the card.
    match &app.model_operation_state {
        ModelOperationState::Ready => {}
        ModelOperationState::Downloading {
            target_model,
            progress,
        } => card = card.push(card_download_progress(target_model, progress)),
        ModelOperationState::Loading {
            target_model,
            status_message,
        } => {
            card = card.push(text::body(format!(
                "Loading {target_model}: {status_message}"
            )));
        }
        ModelOperationState::Error { message } => card = card.push(card_error(message)),
    }

    card_surface(card, true)
}

/// Summary shown in the active-backend card when a model is currently
/// loaded for this backend. Reads as e.g. "Active: whisper-1 · cuda" with
/// an Unload button on the right; the Unload click drops the model but
/// keeps the active backend selected.
pub(super) fn loaded_model_summary(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let device_suffix = if app.current_device.is_empty() || app.current_device == "none" {
        String::new()
    } else {
        format!(" · {}", app.current_device)
    };
    let label = text::body(format!("Active: {}{device_suffix}", app.current_model))
        .class(cosmic::theme::Text::Accent)
        .width(Length::Fill);
    row![
        label,
        // A leading stop glyph fronts the label, mirroring the Load button's
        // play icon so load/unload read as a play/stop pair.
        button::standard("Unload")
            .leading_icon(icons::phosphor_handle(icons::STOP))
            .on_press(Message::UnloadActiveModel),
    ]
    .spacing(spacing.space_xs)
    .align_y(Alignment::Center)
    .into()
}

/// Pure VRAM-fit check for a staged load: given the staged `device`, the
/// model's conservative `estimated_vram_bytes`, and the primary GPU's
/// available bytes, returns `(needed, available)` when a **CUDA** load looks
/// too big to fit. `None` when the device isn't CUDA, the model declares no
/// estimate (online / unknown), no GPU memory is known, or it should fit.
/// Kept free of [`AppModel`] so the rule is directly unit-testable.
pub(super) fn vram_shortfall(
    device: Option<&str>,
    estimated_vram_bytes: u64,
    gpu_available_bytes: Option<u64>,
) -> Option<(u64, u64)> {
    if device != Some("cuda") || estimated_vram_bytes == 0 {
        return None;
    }
    let available = gpu_available_bytes?;
    (estimated_vram_bytes > available).then_some((estimated_vram_bytes, available))
}

/// [`vram_shortfall`] resolved against the current app state: the staged
/// model's VRAM estimate vs. the primary GPU's free memory (falling back to
/// its total when the daemon didn't report free).
pub(super) fn staged_vram_shortfall(backend: &BackendInfo, app: &AppModel) -> Option<(u64, u64)> {
    let model_name = app.staged_model.as_deref()?;
    let model = backend.models.iter().find(|m| m.name == model_name)?;
    let gpu_available = app
        .gpu_info
        .first()
        .map(|g| g.free_bytes.unwrap_or(g.total_bytes));
    vram_shortfall(
        app.staged_device.as_deref(),
        model.estimated_vram_bytes,
        gpu_available,
    )
}

/// Model + device pickers and the Load button, shown in the active-backend
/// card when no model is loaded for this backend. Picking a model stages it
/// (no daemon call); picking a device stages it too; the Load button
/// commits both via `set_device` then `set_model`. For online models
/// (`supported_devices == ["none"]`) the device dropdown is omitted — there
/// is no local compute to pick.
pub(super) fn staged_model_picker<'a>(
    backend: &'a BackendInfo,
    app: &'a AppModel,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();

    // Model dropdown — staged picks live in `app.staged_model`, not loaded.
    let model_names: Vec<String> = backend.models.iter().map(|m| m.name.clone()).collect();
    let staged_model = app.staged_model.as_deref();
    let model_index = staged_model.and_then(|m| model_names.iter().position(|n| n == m));
    let model_names_pick = model_names.clone();
    // Model select takes twice the width of the device select (2:1 flex ratio).
    let model_dropdown = widget::dropdown(model_names, model_index, move |index| {
        Message::StageActiveModel(model_names_pick[index].clone())
    })
    .placeholder("Select model")
    .width(Length::FillPortion(2));

    let mut picker_row = row![model_dropdown]
        .spacing(spacing.space_s)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // Device dropdown — only when (a) a model is staged and (b) that model is
    // not the online sentinel. A single-device model still shows the dropdown
    // (read-only-ish, only one option) for visual consistency with the layout.
    let staged_model_supports: Option<&[String]> = staged_model
        .and_then(|m| backend.models.iter().find(|b| b.name == m))
        .map(|m| m.supported_devices.as_slice());
    let show_device_picker =
        staged_model_supports.is_some_and(|devs| !(devs.len() == 1 && devs[0] == "none"));
    if show_device_picker {
        let devices: Vec<String> = staged_model_supports.unwrap_or(&[]).to_vec();
        let device_index = app
            .staged_device
            .as_deref()
            .and_then(|d| devices.iter().position(|x| x == d));
        let devices_pick = devices.clone();
        let device_dropdown = widget::dropdown(devices, device_index, move |index| {
            Message::StageActiveDevice(devices_pick[index].clone())
        })
        .placeholder("Device")
        .width(Length::FillPortion(1));
        picker_row = picker_row.push(device_dropdown);
    }

    // Load button — enabled only when a model is staged AND (the staged
    // device is set OR the model is the online-only `"none"` one, where
    // there is no device to pick).
    let staged_ok = app.staged_model.is_some()
        && (app.staged_device.is_some()
            || staged_model_supports.is_some_and(|d| d == ["none".to_string()]));
    let load_button = button::suggested("Load model")
        .leading_icon(icons::phosphor_handle(icons::PLAY))
        .on_press_maybe((staged_ok && app.is_model_ready()).then_some(Message::LoadStagedModel));
    picker_row = picker_row.push(load_button);

    // A staged CUDA load whose conservative VRAM estimate exceeds the GPU's
    // available memory gets an advisory yellow warning below the picker.
    if let Some((needed, available)) = staged_vram_shortfall(backend, app) {
        column![picker_row, vram_warning(needed, available)]
            .spacing(spacing.space_xs)
            .into()
    } else {
        picker_row.into()
    }
}

/// In-card download progress (bar + text + Cancel), shown at the bottom of the
/// active-backend card while model files are downloading.
// reason: display-only; the imprecision is cosmetic
#[allow(clippy::cast_precision_loss)]
pub(super) fn card_download_progress<'a>(
    target_model: &'a str,
    progress: &'a DownloadProgress,
) -> Element<'a, Message> {
    let fraction = if progress.percentage < 0.0 || progress.percentage > 100.0 {
        0.0
    } else {
        (progress.percentage / 100.0).clamp(0.0, 1.0)
    };
    let line = format!(
        "Downloading {} ({}/{}): {:.1}%",
        target_model,
        progress.file_index + 1,
        progress.total_files,
        progress.percentage
    );
    let bytes = if progress.total_bytes > 0 {
        let mb = progress.bytes_downloaded as f64 / (1024.0 * 1024.0);
        let total = progress.total_bytes as f64 / (1024.0 * 1024.0);
        format!("{mb:.1} / {total:.1} MB")
    } else {
        String::new()
    };
    column![
        text::body(line),
        widget::progress_bar(0.0..=1.0, fraction.max(0.05)),
        row![
            text::caption(bytes).width(Length::Fill),
            widget::button::destructive("Cancel").on_press(Message::CancelDownload),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(cosmic::theme::spacing().space_xs)
    .into()
}

/// In-card model error: a destructive-colored warning glyph plus the daemon's
/// message. Not dismissible — the error is tied to the backend's state, and
/// clears the moment the user fixes the underlying issue (or picks another
/// model from the dropdown above). The Configure button up in the card header
/// is the user-actionable path.
pub(super) fn card_error(message: &str) -> Element<'_, Message> {
    row![
        icons::phosphor_destructive(icons::WARNING, 18.0),
        text::body(message.to_string()),
    ]
    .spacing(cosmic::theme::spacing().space_xs)
    .align_y(Alignment::Center)
    .into()
}

#[cfg(test)]
mod vram_shortfall_tests {
    //! Pin the staged-load VRAM warning: it fires only for a CUDA target whose
    //! conservative estimate exceeds the GPU's available memory. Anything else
    //! — CPU/Metal, a fitting model, an online/unknown estimate of `0`, or no
    //! GPU info — stays silent.
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    /// A CUDA load bigger than available memory warns, echoing both amounts.
    #[test]
    fn cuda_over_budget_warns_with_amounts() {
        assert_eq!(
            vram_shortfall(Some("cuda"), 48 * GIB, Some(24 * GIB)),
            Some((48 * GIB, 24 * GIB)),
        );
    }

    /// A model that fits — including exactly filling memory — is silent; the
    /// check is strictly-greater.
    #[test]
    fn cuda_within_budget_is_silent() {
        assert_eq!(vram_shortfall(Some("cuda"), 8 * GIB, Some(24 * GIB)), None);
        assert_eq!(vram_shortfall(Some("cuda"), 24 * GIB, Some(24 * GIB)), None);
    }

    /// Online / unknown models declare a `0` estimate and never warn.
    #[test]
    fn zero_estimate_is_silent() {
        assert_eq!(vram_shortfall(Some("cuda"), 0, Some(GIB)), None);
    }

    /// The warning is CUDA-specific — CPU, Metal, and an unset device stay
    /// silent even when the estimate exceeds memory.
    #[test]
    fn non_cuda_devices_are_silent() {
        assert_eq!(vram_shortfall(Some("cpu"), 48 * GIB, Some(24 * GIB)), None);
        assert_eq!(
            vram_shortfall(Some("metal"), 48 * GIB, Some(24 * GIB)),
            None
        );
        assert_eq!(vram_shortfall(None, 48 * GIB, Some(24 * GIB)), None);
    }

    /// Without known GPU memory there's nothing to judge fit against.
    #[test]
    fn no_gpu_info_is_silent() {
        assert_eq!(vram_shortfall(Some("cuda"), 48 * GIB, None), None);
    }
}
