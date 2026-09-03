// SPDX-License-Identifier: GPL-3.0-only
//! The Post-processing card on the Models page.
//!
//! A post-processor rewrites each final transcript — filler removal,
//! punctuation, formatting. It is an ordinary backend model, so once one is
//! selected its card is the same card the active transcription backend gets:
//! the same glyph tile, title block, capability chips, divider, and a
//! stage-then-enable control row that mirrors the transcription card's
//! stage-then-load. The one difference is what the picker offers —
//! post-processors from *every* installed backend, since this selection is
//! independent of the active one.
//!
//! Before a backend is chosen there is no card at all, only a prompt and the
//! button that opens the picker. A card frames a backend that exists; a
//! bordered surface holding nothing but a prompt reads as a card whose contents
//! failed to load.

use cosmic::Element;
use cosmic::iced::widget::row;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use super::language::language_button;
use crate::core::app::AppModel;
use crate::daemon::backends::BackendInfo;
use crate::state::ContextPage;
use crate::state::device_offers::PP_STAGE;
use crate::ui::icons;
use crate::ui::messages::{Message, ModelsPageMessage, PostProcessorMessage, ShellMessage};

use super::active::backend_glyph_tile;
use super::chips::{
    CloudEgress, backend_has_user_url, backend_is_online, capability_chips, count_chip,
    stage_device_support,
};
use super::surface::{
    backend_description, card_divider, card_surface, card_title_block, muted_text_color,
    repo_button,
};

/// The installed backends that serve at least one post-processor, in catalog
/// order.
///
/// This is what the "Select a post-processor" sheet lists — the post-processing
/// twin of the Load-a-backend sheet's full backend list.
pub(crate) fn post_processor_backends(backends: &[BackendInfo]) -> Vec<&BackendInfo> {
    super::roles::backends_for(backends, true)
}

/// The post-processor model names one backend serves, in manifest order.
///
/// The index into this list is what [`PostProcessorMessage::Staged`] carries,
/// so the view and the handler must build it the same way — hence one function,
/// called by both.
pub(crate) fn post_processor_models(backend: &BackendInfo) -> Vec<String> {
    super::roles::models_for(backend, true)
}

/// The backend the card is about: the one the daemon has selected.
///
/// A post-processor selection stores `(model, source)` the way every model
/// selection does, and `source` alone is the "backend chosen, nothing running"
/// state — exactly what `/active_backend` holds for transcription.
fn selected_backend(app: &AppModel) -> Option<&BackendInfo> {
    let source = app.post_processor.source.as_deref()?;
    app.backends.iter().find(|b| b.source == source)
}

/// The Post-processing section: a card once a backend is selected, and a plain
/// prompt line before that.
pub(crate) fn section(app: &AppModel) -> Element<'_, Message> {
    // Nothing installed serves one: say so, rather than offering a picker that
    // opens an empty sheet.
    if post_processor_backends(&app.backends).is_empty() {
        return prompt_row(
            "Install a post-processor from the Library to clean up text before \
             is is typed and remove filler words.",
            None,
        );
    }

    // Installed but none chosen: there is no backend to put in a card, so the
    // section is the prompt and the button that opens the picker sheet.
    let Some(backend) = selected_backend(app) else {
        return unselected_state();
    };

    let spacing = cosmic::theme::spacing();
    let mut card = widget::column::with_capacity(6)
        .spacing(spacing.space_s)
        .push(header(backend, app));

    if let Some(chips) = chip_row(backend, app) {
        card = card.push(chips);
    }

    card = card.push(card_divider()).push(control_row(app, backend));

    // Downloading, loading, or the failure that ended it — shown here when the
    // operation is this stage's, so a post-processor's progress is on the card
    // that started it rather than on the transcription card below the models.
    if let Some(status) = super::active::operation_status(app, PP_STAGE) {
        card = card.push(status);
    }

    // Enabled but not running: transcripts are passing through untouched, and
    // nothing else on the card would say so. `enabled` and `loaded` are
    // separate fields for exactly this case.
    if app.post_processor.is_enabled() && !app.post_processor.loaded {
        card = card.push(notice(
            "The selected model isn't loaded — transcripts are used as they are.",
        ));
    }

    // A failed save surfaces on the card that owns it rather than only reaching
    // the log.
    if let Some(message) = app.action_error_for(crate::state::ErrorScope::PostProcessing) {
        card = card.push(crate::ui::views::common::error_banner(message));
    }

    card_surface(
        card,
        app.post_processor.is_enabled() && app.post_processor.loaded,
    )
}

/// The card header for a selected backend: glyph tile, the backend's name /
/// version / description, then the repo button, Configure and Deselect — the
/// same action set the transcription card carries. Changing backend is ✕ then
/// pick again.
fn header<'a>(backend: &'a BackendInfo, app: &'a AppModel) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let source = backend.source.clone();
    let actions = row![
        repo_button(&source),
        button::standard("Configure").on_press(Message::ModelsPage(
            ModelsPageMessage::OpenBackendConfig(source.clone()),
        )),
        super::surface::deselect_button(
            "Deselect this backend",
            Message::PostProcessor(PostProcessorMessage::Deselect),
        ),
    ]
    .spacing(spacing.space_xs)
    .align_y(Alignment::Center);

    row![
        backend_glyph_tile(),
        card_title_block(
            backend.name.clone(),
            &backend.version,
            backend_description(app, &source),
        ),
        actions,
    ]
    .spacing(spacing.space_s)
    .align_y(Alignment::Center)
    .into()
}

/// Capability chips for the selected backend, plus a count of the
/// post-processors it serves. Same row the transcription card carries, so the
/// two read alike.
fn chip_row<'a>(backend: &'a BackendInfo, app: &AppModel) -> Option<Element<'a, Message>> {
    let spacing = cosmic::theme::spacing();
    let egress = backend_is_online(backend).then(|| CloudEgress {
        hosts: backend.allowed_hosts.as_slice(),
        user_url: backend_has_user_url(backend),
    });
    let compute = stage_device_support(
        app.device_offers.backend(PP_STAGE, &backend.source),
        backend,
    );
    let caps = capability_chips(compute.gpu, compute.cpu, egress, true);
    let count = post_processor_models(backend).len();
    if caps.is_none() && count == 0 {
        return None;
    }
    let mut chips = row![].spacing(spacing.space_xxs).align_y(Alignment::Center);
    if let Some(caps) = caps {
        chips = chips.push(caps);
    }
    if count > 0 {
        let label = if count == 1 {
            "1 post-processor".to_string()
        } else {
            format!("{count} post-processors")
        };
        chips = chips.push(count_chip(label));
    }
    Some(chips.into())
}

/// The model the card's pickers act on: the staged pick when it belongs to this
/// backend, otherwise the selection the daemon remembers.
///
/// Both dropdowns read this one value. They used to derive it separately — the
/// model dropdown falling back to the daemon's selection and the device
/// dropdown not — so after an unload the card showed a model with no device
/// picker beside it, and the device could only be changed by selecting the
/// backend again.
pub(super) fn shown_model(
    staged: Option<&(String, String)>,
    selected_model: Option<&str>,
    source: &str,
) -> Option<String> {
    staged
        .filter(|(_, staged_source)| staged_source == source)
        .map(|(model, _)| model.clone())
        .or_else(|| selected_model.map(ToString::to_string))
}

/// The control row: running state and a Disable button, or the model and
/// device pickers and an Enable button.
///
/// Deliberately the same shape *and the same words* as the transcription card's
/// `loaded_model_summary` / `staged_model_picker` pair — accent "Active:" line
/// with a stop-glyph Unload one way, dropdowns with a play-glyph Load model the
/// other. Both stages run a model in a stage, so both say so: Unload stops
/// processing and keeps the backend, while the header's ✕ clears the selection.
fn control_row<'a>(app: &'a AppModel, backend: &'a BackendInfo) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();

    if app.post_processor.is_enabled() {
        let model = app
            .post_processor
            .model
            .clone()
            .unwrap_or_else(|| "post-processor".to_string());
        // The accelerator it is actually on, the way the transcription card
        // suffixes its active model — absent while it is not loaded.
        let device_suffix = app
            .post_processor
            .device
            .as_deref()
            .filter(|d| !d.is_empty() && *d != "none")
            .map(|d| format!(" \u{00b7} {d}"))
            .unwrap_or_default();
        let label = text::body(format!("Active: {model}{device_suffix}"))
            .class(cosmic::theme::Text::Accent)
            .width(Length::Fill);
        let mut summary = row![label]
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center);
        // The same control the transcription card shows beside its active
        // model, from the same function.
        if let Some(lang_button) = language_button(backend, &model, app) {
            summary = summary.push(lang_button);
        }
        return summary
            .push(
                button::standard("Unload")
                    .leading_icon(icons::phosphor_handle(icons::STOP))
                    .on_press(Message::PostProcessor(PostProcessorMessage::Disable)),
            )
            .into();
    }

    let models = post_processor_models(backend);
    let shown = shown_model(
        app.staged_post_processor.as_ref(),
        app.post_processor.model.as_deref(),
        &backend.source,
    );
    let selected = shown
        .as_deref()
        .and_then(|m| models.iter().position(|n| n == m));
    // Model select takes twice the width of the device select, as on the
    // transcription card.
    let dropdown = widget::dropdown(models, selected, |index| {
        Message::PostProcessor(PostProcessorMessage::Staged(index))
    })
    .placeholder("Select model")
    .width(Length::FillPortion(2));

    let mut picker_row = row![dropdown]
        .spacing(spacing.space_s)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // Device dropdown — keyed on the same model the dropdown above shows, so
    // the two pickers cannot disagree about which model the card is acting on.
    // It renders only once the daemon has answered what that model can run on
    // here, and only when the answer offers something: the answer is the
    // model's declared devices narrowed to what this install and host can do,
    // so an online model (offering nothing) shows no picker.
    let devices: Vec<String> = shown
        .as_deref()
        .and_then(|m| app.device_offers.model(PP_STAGE, &backend.source, m))
        .unwrap_or_default()
        .to_vec();
    if !devices.is_empty() {
        let device_index = app
            .staged_post_processor_device
            .as_deref()
            .and_then(|d| devices.iter().position(|x| x == d));
        let devices_pick = devices.clone();
        let device_dropdown = widget::dropdown(devices, device_index, move |index| {
            Message::PostProcessor(PostProcessorMessage::StagedDevice(
                devices_pick[index].clone(),
            ))
        })
        .placeholder("Device")
        .width(Length::FillPortion(1));
        picker_row = picker_row.push(device_dropdown);
    }

    // Per-model language, inline after the device dropdown — the same position
    // and the same function as the transcription card's staged picker.
    if let Some(model) = shown.as_deref()
        && let Some(lang_button) = language_button(backend, model, app)
    {
        picker_row = picker_row.push(lang_button);
    }

    picker_row
        .push(
            button::suggested("Load model")
                .leading_icon(icons::phosphor_handle(icons::PLAY))
                // Disabled while stage 2 already has an operation in flight,
                // the way the transcription card's is. Stage 1's work does not
                // enter into it: the two stages load independently.
                .on_press_maybe(
                    selected
                        .filter(|_| app.is_model_ready(PP_STAGE))
                        .map(|_| Message::PostProcessor(PostProcessorMessage::Enable)),
                ),
        )
        .into()
}

/// Shown when post-processors are installed but none is chosen: a prompt and
/// the button that opens the picker sheet.
///
/// The "Post-processing" heading above the section already names it, so the
/// line only has to say what a post-processor is for.
fn unselected_state<'a>() -> Element<'a, Message> {
    prompt_row(
        "Optional. Clean up text before it is typed. Remove filler words. Improve punctuation and \
         formatting.",
        Some(
            button::suggested("Select post-processor backend")
                .leading_icon(icons::phosphor_handle(icons::PLAY))
                .on_press(Message::Shell(ShellMessage::ToggleContextPage(
                    ContextPage::SelectPostProcessor,
                )))
                .into(),
        ),
    )
}

/// A card-less line for the two states that name no backend: a muted prompt
/// taking the width, with an optional action opposite it.
fn prompt_row<'a>(caption: &'a str, action: Option<Element<'a, Message>>) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let mut line = widget::row::with_capacity(2)
        .spacing(spacing.space_s)
        .align_y(Alignment::Center)
        .push(
            text::body(caption)
                .class(cosmic::theme::Text::Color(muted_text_color()))
                .width(Length::Fill),
        );
    if let Some(action) = action {
        line = line.push(action);
    }
    line.into()
}

/// A warning glyph plus text, for the not-loaded notice. Mirrors
/// [`card_error`](super::active::card_error), which is private to its module.
fn notice(message: &str) -> Element<'_, Message> {
    row![
        icons::phosphor_destructive(icons::WARNING, 18.0),
        text::body(message.to_string()),
    ]
    .spacing(cosmic::theme::spacing().space_xs)
    .align_y(Alignment::Center)
    .into()
}

/// The "Select post-processor backend" side sheet: one row per installed backend that
/// serves a post-processor. The selected one is flagged; every other row
/// carries a Select button.
///
/// The post-processing twin of
/// [`select_backend_sheet`](super::select_backend_sheet), and deliberately the
/// same shape.
pub fn post_processor_sheet(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let muted = muted_text_color();
    let selected = app.post_processor.source.as_deref();

    let eligible = post_processor_backends(&app.backends);
    let mut col = widget::column::with_capacity(eligible.len() + 1)
        .spacing(spacing.space_xs)
        .width(Length::Fill)
        .push(
            text::caption(
                "Pick which backend cleans up your transcripts. Add and manage backends \
                 in your Library.",
            )
            .class(cosmic::theme::Text::Color(muted)),
        );

    if eligible.is_empty() {
        return col
            .push(text::body(
                "No installed backend provides a post-processor. Open the Library to \
                 install one.",
            ))
            .into();
    }

    for backend in eligible {
        col = col.push(sheet_row(
            backend,
            selected == Some(backend.source.as_str()),
        ));
    }
    col.into()
}

/// One backend row inside the picker sheet: glyph + name + its post-processor
/// models, then either a "Selected" flag or a Select button.
fn sheet_row(backend: &BackendInfo, is_selected: bool) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let muted = muted_text_color();

    let models = post_processor_models(backend);
    let mut meta = widget::column::with_capacity(2)
        .spacing(spacing.space_xxxs)
        .width(Length::Fill)
        .push(text::body(backend.name.clone()));
    if !models.is_empty() {
        meta = meta.push(
            text::caption(models.join(" \u{00b7} ")).class(cosmic::theme::Text::Color(muted)),
        );
    }

    let trailing: Element<'static, Message> = if is_selected {
        text::caption("Selected")
            .class(cosmic::theme::Text::Accent)
            .into()
    } else {
        button::suggested("Select")
            .on_press(Message::PostProcessor(PostProcessorMessage::SelectBackend(
                backend.source.clone(),
            )))
            .into()
    };

    let inner = row![backend_glyph_tile(), meta, trailing]
        .spacing(spacing.space_s)
        .align_y(Alignment::Center);

    widget::container(inner)
        .padding(spacing.space_xs)
        .width(Length::Fill)
        .class(cosmic::theme::Container::custom(move |theme| {
            let cosmic = theme.cosmic();
            let component = &theme.current_container().component;
            cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(component.base.into())),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_s.into(),
                    width: 1.0,
                    color: super::surface::accent_border_color(theme, is_selected),
                },
                ..Default::default()
            }
        }))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::backends::BackendModel;

    fn model(name: &str, role: &str) -> BackendModel {
        BackendModel {
            name: name.into(),
            provider: String::new(),
            supported_devices: vec!["cpu".into()],
            estimated_vram_bytes: 0,
            multilingual: false,
            supported_languages: Vec::new(),
            primary_language: "en".into(),
            realtime: false,
            role: role.into(),
        }
    }

    fn backend(source: &str, name: &str, models: Vec<BackendModel>) -> BackendInfo {
        BackendInfo {
            source: source.into(),
            description: String::new(),
            name: name.into(),
            version: "1.0.0".into(),
            kind: "wasm".into(),
            allowed_hosts: Vec::new(),
            installed_accel: Vec::new(),
            models,
            secrets: Vec::new(),
            options: Vec::new(),
        }
    }

    /// The picker sheet offers only backends that actually serve a
    /// post-processor. Listing one that does not would select a backend whose
    /// model dropdown is then empty, with nothing saying why.
    #[test]
    fn only_backends_serving_a_post_processor_are_offered() {
        let backends = vec![
            backend(
                "github.com/x/stt-only",
                "STT Only",
                vec![model("whisper", "transcription")],
            ),
            backend(
                "github.com/x/combo",
                "Combo",
                vec![
                    model("whisper", "transcription"),
                    model("clean", "post_processor"),
                ],
            ),
        ];

        let offered = post_processor_backends(&backends);
        let names: Vec<&str> = offered.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["Combo"]);
    }

    fn pick(model: &str, source: &str) -> (String, String) {
        (model.to_string(), source.to_string())
    }

    /// The regression: after an unload nothing is staged but the daemon still
    /// remembers the model, and the card shows it. The device dropdown keys
    /// off this same value, so returning `None` here is what left the card
    /// showing a model with no device picker beside it.
    #[test]
    fn the_daemons_selection_is_shown_when_nothing_is_staged() {
        assert_eq!(
            shown_model(None, Some("s1-mini-q4_k_m"), "github.com/x/s1"),
            Some("s1-mini-q4_k_m".to_string()),
        );
    }

    /// A staged pick for this backend wins over the daemon's selection.
    #[test]
    fn a_staged_pick_for_this_backend_wins() {
        let staged = pick("s1-mini-q8_0", "github.com/x/s1");
        assert_eq!(
            shown_model(Some(&staged), Some("s1-mini-q4_k_m"), "github.com/x/s1"),
            Some("s1-mini-q8_0".to_string()),
        );
    }

    /// A pick staged against a different backend is not this card's, so the
    /// daemon's selection shows instead — otherwise the card would offer a
    /// model its own dropdown does not list.
    #[test]
    fn a_staged_pick_for_another_backend_is_ignored() {
        let staged = pick("other-model", "github.com/x/other");
        assert_eq!(
            shown_model(Some(&staged), Some("s1-mini-q4_k_m"), "github.com/x/s1"),
            Some("s1-mini-q4_k_m".to_string()),
        );
    }

    /// A backend chosen with no model picked yet shows nothing, and the device
    /// picker stays hidden with it.
    #[test]
    fn nothing_is_shown_without_a_pick_or_a_selection() {
        assert_eq!(shown_model(None, None, "github.com/x/s1"), None);
    }

    /// A catalog with no post-processor anywhere offers nothing, and the card
    /// falls back to its "none installed" state.
    #[test]
    fn a_catalog_without_post_processors_offers_no_backend() {
        let backends = vec![backend(
            "github.com/x/a",
            "A",
            vec![model("whisper", "transcription")],
        )];
        assert!(post_processor_backends(&backends).is_empty());
    }

    /// The card's dropdown is scoped to the selected backend and lists only its
    /// post-processors — a transcription model here would let the user pick one
    /// the daemon then refuses.
    #[test]
    fn a_backends_models_are_its_post_processors_only() {
        let b = backend(
            "github.com/x/combo",
            "Combo",
            vec![
                model("whisper", "transcription"),
                model("clean-a", "post_processor"),
                model("clean-b", "post_processor"),
            ],
        );
        assert_eq!(post_processor_models(&b), vec!["clean-a", "clean-b"]);
    }

    /// Manifest order is preserved, because the dropdown selection is an index
    /// into this list and a reordering would move the user's pick.
    #[test]
    fn model_order_follows_the_manifest() {
        let b = backend(
            "github.com/x/a",
            "A",
            vec![
                model("zeta", "post_processor"),
                model("alpha", "post_processor"),
            ],
        );
        assert_eq!(post_processor_models(&b), vec!["zeta", "alpha"]);
    }
}
