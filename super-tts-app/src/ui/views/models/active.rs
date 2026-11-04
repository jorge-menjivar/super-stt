// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};
use super_tts_shared::models::protocol::{DownloadProgress, LoadProgress, load_progress};

use crate::core::app::{AppModel, ModelOperationState};
use crate::daemon::backends::BackendInfo;
use crate::ui::icons;
use crate::ui::messages::{
    DownloadMessage, LanguageMessage, Message, ModelsPageMessage, VoiceMessage,
};

use super::chips::{
    CloudEgress, backend_has_user_url, backend_is_online, backend_supports_cpu,
    backend_supports_gpu, capability_chips, count_chip, model_is_online, requirement_warning,
};
use super::fmt::{no_viable_device_warning, vram_warning};
use super::status::unmet_requirements;
use super::surface::{card_divider, card_surface};

/// A soft accent-tinted rounded square holding the "models" brain glyph.
/// `tile` is the square's side, `glyph` the icon size; `radius_medium` picks the
/// larger corner radius for the big empty-state ring vs. the small card tile.
pub(super) fn glyph_tile<'a>(tile: f32, glyph: f32, radius_medium: bool) -> Element<'a, Message> {
    let accent: cosmic::iced::Color = cosmic::theme::active().cosmic().accent.base.into();
    let mut fill = accent;
    fill.a = 0.16;

    widget::container(icons::phosphor_tinted(icons::BRAIN, glyph, accent))
        .center_x(Length::Fixed(tile))
        .center_y(Length::Fixed(tile))
        .class(cosmic::theme::Container::custom(move |theme| {
            let radii = theme.cosmic().corner_radii;
            cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(fill)),
                border: cosmic::iced::Border {
                    radius: if radius_medium {
                        radii.radius_m
                    } else {
                        radii.radius_s
                    }
                    .into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .into()
}

/// The leading glyph tile shared by every backend card: gives each card a
/// consistent anchor on the left that lines up with the two-line name/source
/// block beside it.
pub(super) fn backend_glyph_tile<'a>() -> Element<'a, Message> {
    glyph_tile(40.0, 22.0, false)
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

/// The voice dropdown for the active-backend card.
///
/// A dropdown rather than the search sheet the language control opens, because
/// the two lists are not the same size: a language picker chooses among some
/// sixty tags and needs a search box, where a model offers nine preset voices
/// or a handful of cloned ones and a list is faster than a sheet.
///
/// Row 0 is always the model's own default, so clearing the preference is a
/// pick like any other rather than a second control. A model that declares no
/// `default_voice` — every cloning model — labels that row as the state it
/// really is: nothing chosen, and speech refused until something is.
///
/// Returns `None` when there is nothing to pick from: a model whose voices are
/// all described takes free text and has no list, and one that clones before
/// any clip has been recorded has an empty library. The caller skips the push
/// rather than rendering an empty control.
fn voice_dropdown<'a>(
    backend: &'a BackendInfo,
    selected_model: &str,
    app: &'a AppModel,
) -> Option<Element<'a, Message>> {
    // Stale-block guard, the same one the language button uses: the two values
    // below describe one model, and a card drawing another must not read them.
    if app.voice.target.as_ref() != Some(&(backend.source.clone(), selected_model.to_string())) {
        return None;
    }
    let block = app.voice.resolution.as_ref()?;
    if app.voice.choices.is_empty() {
        return None;
    }

    let default_row = block.default.as_ref().map_or_else(
        || "No cloned voice chosen".to_string(),
        |id| {
            // Named, not just "Default": a user comparing two rows should not
            // have to remember which voice the manifest picked.
            let label = app
                .voice
                .choices
                .iter()
                .find(|v| &v.id == id)
                .map_or(id.as_str(), |v| v.label.as_str());
            format!("{label} · default")
        },
    );
    let mut labels: Vec<String> = vec![default_row];
    // Cloned voices are marked. They sit in the same list as the presets
    // because they are the same choice to the user, but one of them is a
    // recording they made and the other is shipped with the model, and a list
    // that did not say could put two identical labels side by side.
    labels.extend(app.voice.choices.iter().map(|v| {
        if v.kind == "cloned" {
            format!("{} · cloned", v.label)
        } else {
            v.label.clone()
        }
    }));

    // The selected row is the *stored* voice, not the effective one: with no
    // preference set the default row is what is selected, which is exactly what
    // `override: null` means.
    let selected = block.model_override.as_ref().and_then(|id| {
        app.voice
            .choices
            .iter()
            .position(|v| &v.id == id)
            .map(|i| i + 1)
    });

    // The ids are cloned into the closure rather than indexed out of `app` on
    // press: the catalog can be replaced by a refresh between render and click,
    // and an index into a list that has since changed would store the wrong
    // voice.
    let ids: Vec<String> = app.voice.choices.iter().map(|v| v.id.clone()).collect();
    Some(
        widget::dropdown(labels, selected.or(Some(0)), move |index| {
            Message::Voice(VoiceMessage::ModelVoiceSelected(
                index.checked_sub(1).map(|i| ids[i].clone()),
            ))
        })
        .into(),
    )
}

/// A warning for the one state in which the model cannot speak at all: no
/// voice in effect, and no free-text description to fall back on.
///
/// This is what a cloning model looks like before any clip has been recorded,
/// and before this control existed it was invisible — the card said the model
/// was loaded and ready, and the first utterance came back "this model speaks
/// in a cloned voice, so a request has to name one" with nowhere in the app to
/// name one. Saying it on the card turns a failure at speak time into a state
/// the user can see and fix.
///
/// The wording says *why*, not just what, and splits on whether there is
/// anything to pick. A model whose whole voice list is recordings the user
/// made has none of its own — asked for no voice, a cloning checkpoint speaks
/// in whatever the sampler wanders into, which is why the daemon refuses
/// instead — so with a library the fix is one click away on this card, and with
/// an empty one it is a recording on the Voices page. Telling someone who has
/// already recorded a voice to go and record one is the one thing this line
/// must not do.
///
/// A model that takes described voices is exempt: it has nothing to enumerate
/// and needs nothing chosen, so an empty list there is not a problem.
///
/// Rendered on its own line under the controls, never inline beside them: see
/// [`loaded_model_summary`] for why a sentence cannot share that row.
fn voice_warning<'a>(
    backend: &'a BackendInfo,
    selected_model: &str,
    app: &'a AppModel,
) -> Option<Element<'a, Message>> {
    if app.voice.target.as_ref() != Some(&(backend.source.clone(), selected_model.to_string())) {
        return None;
    }
    let block = app.voice.resolution.as_ref()?;
    if block.effective.is_some() || block.kinds.iter().any(|k| k == "described") {
        return None;
    }
    let detail = voice_warning_text(
        !app.voice.choices.is_empty(),
        block.kinds.iter().any(|k| k == "cloned"),
    );
    // The same warning glyph the VRAM notice uses, so the two read as one kind
    // of message rather than two.
    Some(
        row![
            icons::phosphor_warning(icons::WARNING, 16.0),
            text::body(detail),
        ]
        .spacing(cosmic::theme::spacing().space_xxs)
        .align_y(Alignment::Center)
        .into(),
    )
}

/// The wording [`voice_warning`] shows, as a rule of its own: which way out of
/// "nothing to speak in" this user actually has.
///
/// `has_choices` is whether the dropdown above has anything in it, `clones`
/// whether the model takes cloned ids at all. Kept free of [`AppModel`] so the
/// one thing this line must never do — send someone who has already recorded a
/// voice off to record one — is a test rather than a screenshot.
fn voice_warning_text(has_choices: bool, clones: bool) -> &'static str {
    match (has_choices, clones) {
        // Something to pick, and the dropdown offering it is directly above.
        (true, _) => "This model doesn't have built-in voices. Pick a cloned voice above.",
        (false, true) => "This model doesn't have built-in voices. Record one on the Voices page.",
        // Neither a list nor a way to fill one: a manifest that declares preset
        // voices and then ships none. Nothing to send the user to, so the line
        // states the fact and stops.
        (false, false) => "No cloned voice chosen",
    }
}

/// Per-model language trigger button for the active-backend card.
///
/// Rendered inline next to the model/device controls (not on its own row), for
/// the given `selected_model` when the catalog marks it multilingual. Returns
/// `None` for a mono-lingual model so the caller can skip the push entirely.
///
/// The button label reflects the current per-model resolution block when
/// `app.language.model_language_for` matches `(backend.source, selected_model)`;
/// otherwise it falls back to a neutral `"Language"` label (block not yet
/// fetched — stale-block guard). It opens the language picker for this model.
fn language_button<'a>(
    backend: &'a BackendInfo,
    selected_model: &str,
    app: &'a AppModel,
) -> Option<Element<'a, Message>> {
    // Gate: only render for multilingual models (per catalog).
    let catalog_model = backend.models.iter().find(|m| m.name == selected_model)?;
    if !catalog_model.multilingual {
        return None;
    }

    let source = &backend.source;

    // Build the trigger label from the resolution block — but only when the
    // block belongs to this exact (source, model) pair (stale-block guard).
    let label = if app.language.model_language_for.as_ref()
        == Some(&(source.clone(), selected_model.to_string()))
    {
        if let Some(block) = &app.language.model_language {
            let source_str = if block.source.is_empty() {
                "default"
            } else {
                block.source.as_str()
            };
            match (block.effective.as_deref(), source_str) {
                (Some(tag), "override") => crate::ui::languages::friendly_name(tag),
                (Some(tag), "global") => {
                    format!("{} · global", crate::ui::languages::friendly_name(tag))
                }
                _ => format!(
                    "{} · default",
                    crate::ui::languages::friendly_name(&block.primary)
                ),
            }
        } else {
            "Language".to_string()
        }
    } else {
        "Language".to_string()
    };

    Some(
        widget::button::standard(label)
            .on_press(Message::Language(LanguageMessage::OpenLanguagePicker {
                model: Some((source.clone(), selected_model.to_string())),
            }))
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

    // Header: glyph + name/description, then the repo button, Configure, and
    // Deselect. The repo button + description replace the old source caption.
    // Changing backend is ✕ then pick again, as in Super STT: one way in, one
    // way out, and no button that silently swaps what a loaded card is about.
    let description = super::surface::backend_description(app, &source);
    let actions = row![
        super::surface::repo_button(&source),
        button::standard("Configure").on_press(Message::ModelsPage(
            ModelsPageMessage::OpenBackendConfig(source.clone()),
        )),
        super::surface::deselect_button(
            "Deselect this backend",
            Message::ModelsPage(ModelsPageMessage::DeselectBackend),
        ),
    ]
    .spacing(spacing.space_xs)
    .align_y(Alignment::Center);
    let header = row![
        backend_glyph_tile(),
        super::surface::card_title_block(backend.name.clone(), &backend.version, description),
        actions,
    ]
    .spacing(spacing.space_s)
    .align_y(Alignment::Center);

    let mut card = widget::column::with_capacity(6)
        .spacing(spacing.space_s)
        .push(header);

    // Capability chips advertise the backend's compute: GPU / CPU for local
    // models, Cloud for online ones (with the hosts it reaches on hover); a
    // trailing "N models" count chip rounds out the row.
    let egress = online.then(|| CloudEgress {
        hosts: backend.allowed_hosts.as_slice(),
        user_url: backend_has_user_url(backend),
    });
    let mut chip_row = row![].spacing(spacing.space_xxs).align_y(Alignment::Center);
    if let Some(chips) = capability_chips(
        backend_supports_gpu(backend),
        backend_supports_cpu(backend),
        egress,
        true,
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
    // The update chip is the action as well as the sign, which is what lets it
    // work here: this card has no overflow menu, so without it a waiting
    // version would be invisible on the page the user actually sits on. Once
    // pressed it becomes the progress chip in place — the Library card does the
    // same, and an update started from either is watchable from both.
    let registry_map = app.registry.by_source();
    if let Some(s) = app.registry.installs.get(source.as_str()) {
        chip_row = chip_row.push(super::chips::update_progress_chip(s));
    } else if let Some(v) =
        super::chips::update_offer(registry_map.get(source.as_str()).copied(), false)
    {
        chip_row = chip_row.push(super::chips::update_chip(&source, &v, true));
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
            card = card.push(loaded_model_summary(backend, app));
        } else {
            card = card.push(staged_model_picker(backend, app));
        }
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
        ModelOperationState::Provisioning {
            target_model,
            progress,
        } => card = card.push(card_provisioning_progress(target_model, progress)),
        ModelOperationState::Loading {
            target_model,
            status_message,
            load,
        } => {
            card = card.push(match load {
                Some(load) => card_load_progress(target_model, load),
                None => text::body(format!("Loading {target_model}: {status_message}")).into(),
            });
        }
        ModelOperationState::Error { message } => card = card.push(card_error(message)),
    }

    card_surface(card, true)
}

/// The device dropdown for the loaded model, or `None` when there is nothing
/// to offer.
///
/// Rendered on the loaded row, as Super STT's card does, because a running
/// model's device can be changed in place: the daemon reloads it onto the new
/// one. Hidden, that change took Unload, re-pick, Load.
///
/// The list and the selection are the daemon's answer for this model — its
/// declared devices narrowed to what this install and host can do, and the
/// preference it would load with. `None` covers the answer not having arrived
/// and an empty one, which is what an online model gets: neither has anything
/// to pick.
fn running_device_dropdown<'a>(
    backend: &'a BackendInfo,
    app: &'a AppModel,
) -> Option<Element<'a, Message>> {
    let model = app.current_model.as_str();
    let devices: Vec<String> = app.model_devices(&backend.source, model)?.to_vec();
    if devices.is_empty() {
        return None;
    }
    let selected = app
        .model_device_preference(&backend.source, model)
        .and_then(|d| devices.iter().position(|x| x == d));
    let pick = devices.clone();
    Some(
        widget::dropdown(devices, selected, move |index| {
            Message::ModelsPage(ModelsPageMessage::MoveLoadedModel(pick[index].clone()))
        })
        .placeholder("Device")
        .into(),
    )
}

/// What a loaded model is *running* on, when that is not simply the device the
/// picker beside it already shows.
///
/// "cpu · cpu" is noise; a `gpu` choice running on `cuda`, or one that fell
/// back to the CPU, is the thing worth saying. Empty when there is nothing to
/// add, including for an online model, which runs on no local device.
fn resolved_suffix(running: &str, chosen: Option<&str>) -> String {
    if running.is_empty() || running == "none" || Some(running) == chosen {
        return String::new();
    }
    format!(" \u{00b7} {running}")
}

/// Summary shown in the active-backend card when a model is currently
/// loaded for this backend. Reads as e.g. "Active: kokoro-82m · cuda" with
/// the model's device, voice and language beside it and Reload and Unload on
/// the right; Unload frees the device memory but keeps both the backend and
/// the model it was pointed at.
///
/// The device named on the label is the accelerator the load actually resolved
/// to, not the `cpu`/`gpu` preference that asked for it — a `gpu` choice that
/// fell back to the CPU would otherwise have this line tell the user they are
/// on a GPU they are not on.
///
/// Controls on one row, prose on the next. The row has less space than it
/// looks: `page_container` caps the page at 800px however wide the window is,
/// so this row is ~768px and never more, against a device dropdown, a voice
/// dropdown, a language button, Reload and Unload. Reload is a glyph rather
/// than a word for that reason. A sentence pushed in among them is the
/// difference between fitting and not — and what overflows is the *end* of the
/// row, which is Unload, the only way to reach a different model on this page.
/// So [`voice_warning`] goes underneath, the way [`no_viable_device_warning`]
/// and [`vram_warning`] already do for the idle half of this card. The accent
/// label is the one `Length::Fill` here, so when the fixed controls do crowd
/// it, what gives is the model name wrapping rather than a button leaving the
/// card.
pub(super) fn loaded_model_summary<'a>(
    backend: &'a BackendInfo,
    app: &'a AppModel,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let label = text::body(format!(
        "Active: {}{}",
        app.current_model,
        resolved_suffix(
            &app.current_device,
            app.model_device_preference(&backend.source, &app.current_model)
        )
    ))
    .class(cosmic::theme::Text::Accent)
    .width(Length::Fill);
    // Top-aligned, not centred: the label is the one thing here that can grow
    // to two lines, and centring against a wrapped one drags every control down
    // half a line with it. Anchored to the top, the row keeps its shape and the
    // second line hangs below it.
    let mut summary = row![label]
        .spacing(spacing.space_xs)
        .align_y(Alignment::Start);
    // The same device picker the unloaded card shows, in the same place.
    // Changing it now moves the running model rather than staging a choice for
    // a Load that is not coming.
    if let Some(devices) = running_device_dropdown(backend, app) {
        summary = summary.push(devices);
    }
    // Per-model voice and language, inline before Reload. Voice first: it is
    // the one a user changes between utterances, and the one that decides
    // whether a cloning model can speak at all.
    if let Some(voice) = voice_dropdown(backend, &app.current_model, app) {
        summary = summary.push(voice);
    }
    if let Some(lang_button) = language_button(backend, &app.current_model, app) {
        summary = summary.push(lang_button);
    }
    // Reload sits before Unload because it is the gentler of the two: it brings
    // the same model back up on the same device, for the case the daemon cannot
    // see — a secret edited by another tool, a model file replaced underneath
    // it. Without it the only way to pick that change up is Unload, re-pick,
    // Load. A glyph with its word in the tooltip, like the header's ✕, so the
    // device picker has room. Disabled while a switch is in flight, so a second
    // click cannot race the first.
    summary = summary.push(super::surface::rounded_tooltip(
        widget::button::icon(icons::phosphor_handle(icons::ARROWS_CLOCKWISE))
            .class(cosmic::theme::Button::Standard)
            .on_press_maybe(
                app.is_model_ready()
                    .then_some(Message::ModelsPage(ModelsPageMessage::ReloadActiveModel)),
            ),
        text::body("Reload this model"),
        widget::tooltip::Position::Bottom,
    ));
    // A leading stop glyph fronts the Unload label, mirroring the Load button's
    // play icon so load/unload read as a play/stop pair.
    let summary = summary.push(
        button::standard("Unload")
            .leading_icon(icons::phosphor_handle(icons::STOP))
            .on_press(Message::ModelsPage(ModelsPageMessage::UnloadActiveModel)),
    );

    // The "nothing to speak in" line, below the row it is about — the dropdown
    // that answers it is the control directly above.
    match voice_warning(backend, &app.current_model, app) {
        Some(warning) => column![summary, warning].spacing(spacing.space_xs).into(),
        None => summary.into(),
    }
}

/// Pure VRAM-fit check for a staged load: given the staged `device`, the
/// model's conservative `estimated_vram_bytes`, and the primary GPU's
/// available bytes, returns `(needed, available)` when a **GPU** load looks
/// too big to fit. `None` when the device isn't `gpu`, the model declares no
/// estimate (online / unknown), no GPU memory is known, or it should fit.
/// Kept free of [`AppModel`] so the rule is directly unit-testable.
pub(super) fn vram_shortfall(
    device: Option<&str>,
    estimated_vram_bytes: u64,
    gpu_available_bytes: Option<u64>,
) -> Option<(u64, u64)> {
    if device != Some("gpu") || estimated_vram_bytes == 0 {
        return None;
    }
    let available = gpu_available_bytes?;
    (estimated_vram_bytes > available).then_some((estimated_vram_bytes, available))
}

/// [`vram_shortfall`] resolved against the current app state: the staged
/// model's VRAM estimate vs. the primary GPU's free memory (falling back to
/// its total when the daemon didn't report free).
pub(super) fn staged_vram_shortfall(backend: &BackendInfo, app: &AppModel) -> Option<(u64, u64)> {
    let model_name = app.models_page.staged_model.as_deref()?;
    let model = backend.models.iter().find(|m| m.name == model_name)?;
    let gpu_available = app
        .gpu_info
        .first()
        .map(|g| g.free_bytes.unwrap_or(g.total_bytes));
    vram_shortfall(
        app.models_page.staged_device.as_deref(),
        model.estimated_vram_bytes,
        gpu_available,
    )
}

/// Model + device pickers and the Load button, shown in the active-backend
/// card when no model is loaded for this backend. Picking a model stages it and
/// asks the daemon what that model's device and language are; picking a device
/// stages it; the Load button writes the device against the model and then
/// points the stage at it.
///
/// The device dropdown lists what the daemon offers *this model* — its declared
/// devices narrowed to the accelerators the installed asset shipped and to what
/// this host actually has — and that answer arrives a round-trip after the pick.
/// Until it does the stage's own list stands in, which is the union over the
/// backend's models and so never hides a device the model has; it can offer one
/// the model lacks, which is why Load stays disabled until the narrow answer
/// confirms the staged device.
///
/// An empty narrow list is the model's real answer and hides the dropdown: an
/// online model, or one with no viable device on this install. The two are not
/// the same state, and `model_is_online` is what tells them apart.
pub(super) fn staged_model_picker<'a>(
    backend: &'a BackendInfo,
    app: &'a AppModel,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();

    // Model dropdown — staged picks live in `app.models_page.staged_model`, not loaded.
    let model_names: Vec<String> = backend.models.iter().map(|m| m.name.clone()).collect();
    let staged_model = app.models_page.staged_model.as_deref();
    let model_index = staged_model.and_then(|m| model_names.iter().position(|n| n == m));
    let model_names_pick = model_names.clone();
    // Model select takes twice the width of the device select (2:1 flex ratio).
    let model_dropdown = widget::dropdown(model_names, model_index, move |index| {
        Message::ModelsPage(ModelsPageMessage::StageActiveModel(
            model_names_pick[index].clone(),
        ))
    })
    .placeholder("Select model")
    .width(Length::FillPortion(2));

    let mut picker_row = row![model_dropdown]
        .spacing(spacing.space_s)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // The daemon's per-model device answer, once it has arrived for this exact
    // pair. `None` is a distinct state from an empty list — it is the window
    // between staging a model and the answer landing — and collapsing the two
    // would flash "this model can't be loaded here" on every single pick.
    let staged_online = staged_model.is_some_and(|m| model_is_online(backend, m));
    let offered: Option<&[String]> =
        staged_model.and_then(|m| app.model_devices(&backend.source, m));

    // Device dropdown — the model's own list once known, the stage's broader
    // one until then so the control doesn't appear a beat after the model
    // dropdown it sits beside. An online model needs no device at all and gets
    // no dropdown even while the answer is outstanding, since the catalog
    // already settles that question.
    let listed: &[String] = offered.unwrap_or(app.stage_devices.as_slice());
    if !staged_online && !listed.is_empty() {
        let devices: Vec<String> = listed.to_vec();
        let device_index = app
            .models_page
            .staged_device
            .as_deref()
            .and_then(|d| devices.iter().position(|x| x == d));
        let devices_pick = devices.clone();
        let device_dropdown = widget::dropdown(devices, device_index, move |index| {
            Message::ModelsPage(ModelsPageMessage::StageActiveDevice(
                devices_pick[index].clone(),
            ))
        })
        .placeholder("Device")
        .width(Length::FillPortion(1));
        picker_row = picker_row.push(device_dropdown);
    }

    // Per-model voice, then language, inline after the device dropdown — the
    // same order they sit in once the model is loaded.
    //
    // Before Load, not only after: staging a model already reads its voice
    // (`StageActiveModel` batches `load_model_voice` with the language and
    // device fetches for exactly this), and a cloning model is the case that
    // needs it — its first utterance is refused until a voice is named, so
    // being told after the checkpoint is resident is being told a load too
    // late. The daemon stores the pick whether or not the model is loaded.
    if let Some(model) = staged_model
        && let Some(voice) = voice_dropdown(backend, model, app)
    {
        picker_row = picker_row.push(voice);
    }
    if let Some(model) = staged_model
        && let Some(lang_button) = language_button(backend, model, app)
    {
        picker_row = picker_row.push(lang_button);
    }

    // Load button — enabled only when a model is staged AND (the online
    // sentinel, which needs no device at all, OR the staged device is one the
    // daemon has confirmed for *this* model). Waiting for that confirmation is
    // what makes the broader stage-wide fallback above safe to offer: a device
    // picked from it that this model cannot use never reaches the daemon.
    //
    // The daemon's list is empty for two different reasons — an online model,
    // and a local model this install can run on no device (a GPU-only model
    // with only a CPU asset, say). `model_is_online` is what tells them apart;
    // conflating them would enable Load with nothing to stage it onto.
    let no_viable_device = !staged_online && offered.is_some_and(<[String]>::is_empty);
    let staged_device_confirmed = offered
        .zip(app.models_page.staged_device.as_deref())
        .is_some_and(|(devices, staged)| devices.iter().any(|d| d == staged));
    let staged_ok = staged_model.is_some() && (staged_online || staged_device_confirmed);
    let load_button = button::suggested("Load model")
        .leading_icon(icons::phosphor_handle(icons::PLAY))
        .on_press_maybe(
            (staged_ok && app.is_model_ready())
                .then_some(Message::ModelsPage(ModelsPageMessage::LoadStagedModel)),
        );
    picker_row = picker_row.push(load_button);

    // Below the picker, one line each. The device advisory ("can't be loaded
    // here", blocking — Load is already disabled above) and the VRAM one (a
    // staged GPU load whose conservative estimate exceeds free memory,
    // non-blocking) can't both fire: the VRAM check requires a staged `gpu`
    // device, which `no_viable_device` rules out.
    //
    // The voice advisory is independent of both and can join either. A cloning
    // model with nothing chosen is worth saying whether or not the device it
    // would load onto is also a problem — they are two different things to fix,
    // and showing one of them only after the other is resolved would hide the
    // one the user can fix right now.
    let mut advisories: Vec<Element<Message>> = Vec::new();
    if no_viable_device {
        advisories.push(no_viable_device_warning(staged_model.unwrap_or_default()));
    } else if let Some((needed, available)) = staged_vram_shortfall(backend, app) {
        advisories.push(vram_warning(needed, available));
    }
    if let Some(model) = staged_model
        && let Some(warning) = voice_warning(backend, model, app)
    {
        advisories.push(warning);
    }
    if advisories.is_empty() {
        return picker_row.into();
    }
    let mut stack = widget::column::with_capacity(advisories.len() + 1)
        .spacing(spacing.space_xs)
        .push(picker_row);
    for advisory in advisories {
        stack = stack.push(advisory);
    }
    stack.into()
}

/// How the card names the phase the daemon reports. `verifying` is checking
/// files already on disk against their checksums; everything else the daemon
/// routes to this card is bytes coming off the network, and an unrecognised
/// status reads as a download because that is what every status other than
/// `verifying` has ever meant here.
fn provisioning_verb(status: &str) -> &'static str {
    if status == "verifying" {
        "Verifying"
    } else {
        "Downloading"
    }
}

/// In-card provisioning progress (bar + text + Cancel), shown at the bottom of
/// the active-backend card while the model's files are being verified or
/// downloaded.
///
/// The verb comes from the daemon's phase, and that distinction is the whole
/// point: loading a model whose files are all present still walks every file to
/// check its checksum — seconds of real work for multi-GB weights — and calling
/// that "Downloading" is what made every cached load look like it was fetching
/// files the user already had. Both phases are byte-tracked the same way, so
/// they share one bar.
// reason: display-only; the imprecision is cosmetic
#[allow(clippy::cast_precision_loss)]
pub(super) fn card_provisioning_progress<'a>(
    target_model: &'a str,
    progress: &'a DownloadProgress,
) -> Element<'a, Message> {
    let fraction = if progress.percentage < 0.0 || progress.percentage > 100.0 {
        0.0
    } else {
        (progress.percentage / 100.0).clamp(0.0, 1.0)
    };
    let line = format!(
        "{} {} ({}/{}): {:.1}%",
        provisioning_verb(&progress.status),
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
        widget::determinate_linear(fraction.max(0.05)).width(Length::Fill),
        row![
            text::caption(bytes).width(Length::Fill),
            widget::button::destructive("Cancel")
                .on_press(Message::Download(DownloadMessage::CancelDownload)),
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
/// The card's title while a backend loads: "Initial setup" when the backend
/// says this load pays a one-time cost, so the user knows the wait will not
/// repeat, and the model being loaded otherwise.
fn load_title(load: &LoadProgress, target_model: &str) -> String {
    if load.phase.as_deref() == Some(load_progress::phase::INITIAL_SETUP) {
        "Initial setup".to_string()
    } else {
        format!("Loading {target_model}")
    }
}

/// What the backend says it is doing, in words. An id this app does not know
/// is still a load in progress, so it reads as one rather than as an error.
fn load_step(step: Option<&str>) -> &'static str {
    match step {
        Some(load_progress::step::LOADING_WEIGHTS) => "Loading weights",
        Some(load_progress::step::BUILDING_KERNELS) => "Building kernels",
        Some(load_progress::step::WARMING_UP) => "Warming up",
        _ => "Loading",
    }
}

/// In-card load progress, as the backend reports it: a title, the step, and a
/// bar when the backend can say how far through the step it is.
///
/// A backend that compiles its GPU kernels on the first load spends minutes
/// there. Before backends reported it, this card said "Loading model into
/// memory" for all of it, which reads as stuck.
fn card_load_progress<'a>(target_model: &str, load: &LoadProgress) -> Element<'a, Message> {
    let title = text::body(load_title(load, target_model));
    let step = load_step(load.step.as_deref());
    match load.progress {
        Some(fraction) => column![
            title,
            row![
                text::caption(step).width(Length::Fill),
                text::caption(format!("{:.0}%", fraction * 100.0)),
            ],
            widget::determinate_linear(fraction.clamp(0.0, 1.0).max(0.02)).width(Length::Fill),
        ]
        .spacing(cosmic::theme::spacing().space_xs)
        .into(),
        None => column![title, text::caption(step)]
            .spacing(cosmic::theme::spacing().space_xs)
            .into(),
    }
}

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
    //! Pin the staged-load VRAM warning: it fires only for a `gpu` target
    //! whose conservative estimate exceeds the GPU's available memory.
    //! Anything else — CPU, a fitting model, an online/unknown estimate of
    //! `0`, or no GPU info — stays silent.
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    /// A GPU load bigger than available memory warns, echoing both amounts.
    #[test]
    fn gpu_over_budget_warns_with_amounts() {
        assert_eq!(
            vram_shortfall(Some("gpu"), 48 * GIB, Some(24 * GIB)),
            Some((48 * GIB, 24 * GIB)),
        );
    }

    /// A model that fits — including exactly filling memory — is silent; the
    /// check is strictly-greater.
    #[test]
    fn gpu_within_budget_is_silent() {
        assert_eq!(vram_shortfall(Some("gpu"), 8 * GIB, Some(24 * GIB)), None);
        assert_eq!(vram_shortfall(Some("gpu"), 24 * GIB, Some(24 * GIB)), None);
    }

    /// Online / unknown models declare a `0` estimate and never warn.
    #[test]
    fn zero_estimate_is_silent() {
        assert_eq!(vram_shortfall(Some("gpu"), 0, Some(GIB)), None);
    }

    /// The warning is GPU-specific — CPU and an unset device stay silent
    /// even when the estimate exceeds memory.
    #[test]
    fn non_gpu_devices_are_silent() {
        assert_eq!(vram_shortfall(Some("cpu"), 48 * GIB, Some(24 * GIB)), None);
        assert_eq!(vram_shortfall(None, 48 * GIB, Some(24 * GIB)), None);
    }

    /// Without known GPU memory there's nothing to judge fit against.
    #[test]
    fn no_gpu_info_is_silent() {
        assert_eq!(vram_shortfall(Some("gpu"), 48 * GIB, None), None);
    }
}

#[cfg(test)]
mod resolved_suffix_tests {
    //! The loaded label names the accelerator only when the picker beside it
    //! does not already say it.
    use super::resolved_suffix;

    /// A `gpu` preference running on CUDA says where it actually is.
    #[test]
    fn an_accelerator_the_picker_does_not_name_is_shown() {
        assert_eq!(resolved_suffix("cuda", Some("gpu")), " \u{00b7} cuda");
    }

    /// So is a fallback: a `gpu` preference that came up on the CPU.
    #[test]
    fn a_fallback_to_the_cpu_is_shown() {
        assert_eq!(resolved_suffix("cpu", Some("gpu")), " \u{00b7} cpu");
    }

    /// "cpu · cpu" is the picker said twice.
    #[test]
    fn the_device_the_picker_shows_is_not_repeated() {
        assert_eq!(resolved_suffix("cpu", Some("cpu")), "");
    }

    /// Nothing running locally, or not yet read back, adds nothing.
    #[test]
    fn no_local_device_adds_nothing() {
        assert_eq!(resolved_suffix("", Some("gpu")), "");
        assert_eq!(resolved_suffix("none", None), "");
    }

    /// With no answer for the picker yet, the accelerator still shows.
    #[test]
    fn an_unanswered_picker_still_names_the_accelerator() {
        assert_eq!(resolved_suffix("cuda", None), " \u{00b7} cuda");
    }
}

#[cfg(test)]
mod voice_warning_text_tests {
    //! Pin which way out the "nothing to speak in" line offers. The dropdown
    //! and the Voices page are different answers, and offering the wrong one
    //! is the whole failure mode: a user with a library told to go and record.
    use super::voice_warning_text;

    /// Which verb the line reaches for, lowercased. The wording is meant to be
    /// edited; which way out it points is not, so that is all these assert on.
    fn routes(has_choices: bool, clones: bool) -> (String, bool, bool) {
        let text = voice_warning_text(has_choices, clones).to_lowercase();
        let picks = text.contains("pick");
        let records = text.contains("record");
        (text, picks, records)
    }

    #[test]
    fn a_library_sends_the_user_to_the_dropdown_above() {
        for clones in [true, false] {
            let (text, picks, records) = routes(true, clones);
            assert!(picks, "{text}");
            assert!(!records, "{text}");
        }
    }

    #[test]
    fn an_empty_library_sends_a_cloning_model_to_the_voices_page() {
        let (text, _, records) = routes(false, true);
        assert!(records, "{text}");
        assert!(text.contains("voices page"), "{text}");
    }

    /// Preset voices declared and none shipped: there is nowhere to send
    /// anyone, so the line says what is true and offers nothing.
    #[test]
    fn nothing_to_pick_and_nothing_to_record_just_states_it() {
        let (text, picks, records) = routes(false, false);
        assert!(!picks, "{text}");
        assert!(!records, "{text}");
    }
}

#[cfg(test)]
mod provisioning_verb_tests {
    //! The card takes its verb from the daemon's phase. A load whose files are
    //! all on disk spends its whole life in `verifying` — calling that
    //! "Downloading" is what had every cached load showing a download bar for
    //! files the user already had.
    use super::provisioning_verb;

    #[test]
    fn verifying_says_so_rather_than_claiming_a_download() {
        assert_eq!(provisioning_verb("verifying"), "Verifying");
    }

    #[test]
    fn downloading_is_still_a_download() {
        assert_eq!(provisioning_verb("downloading"), "Downloading");
    }

    /// An unrecognised phase reads as a download: every status the daemon has
    /// ever routed to this card other than `verifying` is one.
    #[test]
    fn an_unknown_phase_falls_back_to_downloading() {
        assert_eq!(provisioning_verb("something_new"), "Downloading");
    }
}

#[cfg(test)]
mod load_progress_tests {
    //! The words the load card uses for what a backend reports.
    use super::{LoadProgress, load_step, load_title};

    /// A first load says it is setup, so the user knows the wait is a one-off;
    /// any other load names the model.
    #[test]
    fn a_first_load_is_titled_as_setup() {
        let setup = LoadProgress {
            phase: Some("initial_setup".to_string()),
            ..LoadProgress::default()
        };
        assert_eq!(load_title(&setup, "qwen3-tts-0.6b"), "Initial setup");
        let ordinary = LoadProgress {
            phase: Some("loading".to_string()),
            ..LoadProgress::default()
        };
        assert_eq!(
            load_title(&ordinary, "qwen3-tts-0.6b"),
            "Loading qwen3-tts-0.6b"
        );
        assert_eq!(
            load_title(&LoadProgress::default(), "qwen3-tts-0.6b"),
            "Loading qwen3-tts-0.6b"
        );
    }

    /// Every id in the contract's vocabulary has its words, and one this
    /// build does not know still reads as a load.
    #[test]
    fn each_step_reads_as_words() {
        assert_eq!(load_step(Some("loading_weights")), "Loading weights");
        assert_eq!(load_step(Some("building_kernels")), "Building kernels");
        assert_eq!(load_step(Some("warming_up")), "Warming up");
        assert_eq!(load_step(Some("defragmenting_vram")), "Loading");
        assert_eq!(load_step(None), "Loading");
    }
}
