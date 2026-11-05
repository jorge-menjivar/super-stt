// SPDX-License-Identifier: GPL-3.0-only
//! The section of a stage the app reads back from the daemon
//! ([`BlockStage`]): post-processing's on the Models page.
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
//! What names the stage on the section, from its prompts to its picker sheet,
//! is in [`Copy`]; the rest is the same for every such stage.
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
use crate::state::{BlockStage, ContextPage};
use crate::ui::icons;
use crate::ui::messages::{Message, ModelsPageMessage, ShellMessage, StageMessage};

use super::active::backend_glyph_tile;
use super::chips::{
    CloudEgress, backend_has_user_url, backend_is_online, capability_chips, count_chip,
    model_is_online, stage_device_support,
};
use super::surface::{
    backend_description, card_divider, card_surface, card_title_block, muted_text_color,
    repo_button,
};

/// What a section says that names its stage.
struct Copy {
    /// The prompt when nothing installed serves the stage.
    none_installed: &'static str,
    /// The prompt when something does but none is selected.
    unselected: &'static str,
    /// The button that opens the picker sheet, and the sheet's title.
    select: &'static str,
    /// The notice for a stage switched on whose model is not running.
    not_loaded: &'static str,
    /// The count chip's noun, singular and plural.
    count: (&'static str, &'static str),
    /// The line above the sheet's rows.
    sheet_caption: &'static str,
    /// The sheet when nothing installed serves the stage.
    sheet_empty: &'static str,
}

fn copy(stage: BlockStage) -> Copy {
    match stage {
        BlockStage::PostProcessing => Copy {
            none_installed: "Install a post-processor from the Library to clean up text before \
                             it is typed and remove filler words.",
            unselected: "Optional. Clean up text before it is typed. Remove filler words. \
                         Improve punctuation and formatting.",
            select: "Select post-processor backend",
            not_loaded: "The selected model isn't loaded — transcripts are used as they are.",
            count: ("post-processor", "post-processors"),
            sheet_caption: "Pick which backend cleans up your transcripts. Add and manage \
                            backends in your Library.",
            sheet_empty: "No installed backend provides a post-processor. Open the Library \
                          to install one.",
        },
        BlockStage::Synthesis => Copy {
            none_installed: "Install a text-to-speech backend from the Library to have text \
                             read aloud.",
            unselected: "Select a backend to start speaking.",
            select: "Select synthesis backend",
            not_loaded: "The selected model isn't loaded. Text can't be spoken until it is.",
            count: ("model", "models"),
            sheet_caption: "Pick which backend powers speech. Add and manage backends in your \
                            Library.",
            sheet_empty: "No installed backend provides text to speech. Open the Library to \
                          install one.",
        },
    }
}

/// The title of `stage`'s picker sheet.
pub fn sheet_title(stage: BlockStage) -> &'static str {
    copy(stage).select
}

/// The backends that can fill `stage`, each already narrowed to the models it
/// serves there: the daemon's answer, from
/// `GET /{feature}/pipeline/{number}/backend/list`.
///
/// `None` while the answer is in flight, which the sheet draws differently from
/// an empty answer: one is loading, the other is "install one".
pub(crate) fn stage_backends(app: &AppModel, stage: BlockStage) -> Option<&[BackendInfo]> {
    app.stage_catalog.backends(stage.stage())
}

/// The model names one backend serves in `stage`, in manifest order.
fn stage_models(app: &AppModel, stage: BlockStage, backend: &BackendInfo) -> Vec<String> {
    app.stage_catalog.models(stage.stage(), &backend.source)
}

/// The backend the card is about: the one the daemon has selected.
///
/// A selection stores `(model, source)` the way every model selection does,
/// and `source` alone is the "backend chosen, nothing running" state — exactly
/// what `/active_backend` holds for transcription.
fn selected_backend(app: &AppModel, stage: BlockStage) -> Option<&BackendInfo> {
    let source = app.block(stage).source.as_deref()?;
    app.backends.iter().find(|b| b.source == source)
}

/// `stage`'s section: a card once a backend is selected, and a plain prompt
/// line before that.
pub(crate) fn section(app: &AppModel, stage: BlockStage) -> Element<'_, Message> {
    let copy = copy(stage);
    let block = app.block(stage);
    // Nothing installed serves one: say so, rather than offering a picker that
    // opens an empty sheet.
    // Only once the daemon has answered: while the question is in flight the
    // section falls through to its normal prompt rather than flashing "install
    // one" and then replacing it.
    if stage_backends(app, stage).is_some_and(<[BackendInfo]>::is_empty) {
        return prompt_row(copy.none_installed, None);
    }

    // Installed but none chosen: there is no backend to put in a card, so the
    // section is the prompt and the button that opens the picker sheet.
    let Some(backend) = selected_backend(app, stage) else {
        return unselected_state(stage, &copy);
    };

    let spacing = cosmic::theme::spacing();
    let mut card = widget::column::with_capacity(6)
        .spacing(spacing.space_s)
        .push(header(stage, backend, app));

    if let Some(chips) = chip_row(stage, &copy, backend, app) {
        card = card.push(chips);
    }

    card = card
        .push(card_divider())
        .push(control_row(app, stage, backend));

    // The one state in which a synthesis model cannot speak at all, on its own
    // line: a sentence cannot share the controls' row.
    if stage == BlockStage::Synthesis
        && let Some(model) = card_model(app, stage, backend)
        && let Some(warning) = super::voice::voice_warning(backend, &model, app)
    {
        card = card.push(warning);
    }

    // Downloading, loading, or the failure that ended it — shown here when the
    // operation is this stage's, so its progress is on the card that started
    // it rather than on the transcription card above it.
    if let Some(status) = super::active::operation_status(app, stage.stage()) {
        card = card.push(status);
    }

    // Enabled but not running: transcripts are passing through untouched, and
    // nothing else on the card would say so. `enabled` and `loaded` are
    // separate fields for exactly this case.
    if block.enabled && !block.loaded {
        card = card.push(notice(copy.not_loaded));
    }

    // A failed save surfaces on the card that owns it rather than only reaching
    // the log.
    if let Some(message) = app.action_error_for(stage.error_scope()) {
        card = card.push(crate::ui::views::common::error_banner(message));
    }

    card_surface(card, block.enabled && block.loaded)
}

/// The card header for a selected backend: glyph tile, the backend's name /
/// version / description, then the repo button, Configure and Deselect — the
/// same action set the transcription card carries. Changing backend is ✕ then
/// pick again.
fn header<'a>(
    stage: BlockStage,
    backend: &'a BackendInfo,
    app: &'a AppModel,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let source = backend.source.clone();
    let actions = row![
        repo_button(&source),
        button::standard("Configure").on_press(Message::ModelsPage(
            ModelsPageMessage::OpenBackendConfig(source.clone()),
        )),
        super::surface::deselect_button(
            "Deselect this backend",
            Message::Stage(StageMessage::DeselectBackend {
                stage: stage.stage()
            }),
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
fn chip_row<'a>(
    stage: BlockStage,
    copy: &Copy,
    backend: &'a BackendInfo,
    app: &AppModel,
) -> Option<Element<'a, Message>> {
    let spacing = cosmic::theme::spacing();
    let egress = backend_is_online(backend).then(|| CloudEgress {
        hosts: backend.allowed_hosts.as_slice(),
        user_url: backend_has_user_url(backend),
    });
    let compute = stage_device_support(
        app.device_offers.backend(stage.stage(), &backend.source),
        backend,
    );
    let caps = capability_chips(compute.gpu, compute.cpu, egress, true);
    let count = stage_models(app, stage, backend).len();
    if caps.is_none() && count == 0 {
        return None;
    }
    let mut chips = row![].spacing(spacing.space_xxs).align_y(Alignment::Center);
    if let Some(caps) = caps {
        chips = chips.push(caps);
    }
    if count > 0 {
        let (one, many) = copy.count;
        let label = if count == 1 {
            format!("1 {one}")
        } else {
            format!("{count} {many}")
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
    staged: Option<&crate::state::staged_picks::StagedPick>,
    selected_model: Option<&str>,
) -> Option<String> {
    staged
        .map(|pick| pick.model.clone())
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
fn control_row<'a>(
    app: &'a AppModel,
    stage: BlockStage,
    backend: &'a BackendInfo,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let block = app.block(stage);

    if block.enabled {
        let model = block
            .model
            .clone()
            .unwrap_or_else(|| stage.noun().to_string());
        // What it is actually running on, the way the transcription card
        // suffixes its active model — and only when that differs from the
        // device the picker beside it already shows.
        let device_suffix = super::active::resolved_suffix(
            block.running_device().unwrap_or_default(),
            app.staged_picks.device(stage.stage()),
        );
        let label = text::body(format!("Active: {model}{device_suffix}"))
            .class(cosmic::theme::Text::Accent)
            .width(Length::Fill);
        let mut summary = row![label]
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center);
        // The same picker the disabled card shows, in the same place: a running
        // post-processor's device can be changed in place too.
        if let Some(devices) =
            super::active::device_dropdown(stage.stage(), &backend.source, &model, app)
        {
            summary = summary.push(devices);
        }
        // Voice before language: it is the one a user changes between
        // utterances, and the one that decides whether a cloning model can
        // speak at all.
        if stage == BlockStage::Synthesis
            && let Some(voice) = super::voice::voice_dropdown(backend, &model, app)
        {
            summary = summary.push(voice);
        }
        // The same control the transcription card shows beside its active
        // model, from the same function.
        if let Some(lang_button) = language_button(stage.stage(), backend, &model, app) {
            summary = summary.push(lang_button);
        }
        if stage == BlockStage::Synthesis {
            summary = summary.push(reload_button(app, stage));
        }
        return summary
            .push(
                button::standard("Unload")
                    .leading_icon(icons::phosphor_handle(icons::STOP))
                    .on_press(Message::Stage(StageMessage::Unload {
                        stage: stage.stage(),
                    })),
            )
            .into();
    }

    let models = stage_models(app, stage, backend);
    let shown = shown_model(
        app.staged_picks.for_backend(stage.stage(), &backend.source),
        block.model.as_deref(),
    );
    let selected = shown
        .as_deref()
        .and_then(|m| models.iter().position(|n| n == m));
    // Model select takes twice the width of the device select, as on the
    // transcription card.
    let models_pick = models.clone();
    let dropdown = widget::dropdown(models, selected, move |index| {
        Message::Stage(StageMessage::StageModel {
            stage: stage.stage(),
            model: models_pick[index].clone(),
        })
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
    if let Some(model) = shown.as_deref()
        && let Some(devices) =
            super::active::device_dropdown(stage.stage(), &backend.source, model, app)
    {
        picker_row = picker_row.push(devices);
    }

    // The voice, for the same model, before its language as on the running
    // card.
    if stage == BlockStage::Synthesis
        && let Some(model) = shown.as_deref()
        && let Some(voice) = super::voice::voice_dropdown(backend, model, app)
    {
        picker_row = picker_row.push(voice);
    }

    // Per-model language, inline after the device dropdown — the same position
    // and the same function as the transcription card's staged picker.
    if let Some(model) = shown.as_deref()
        && let Some(lang_button) = language_button(stage.stage(), backend, model, app)
    {
        picker_row = picker_row.push(lang_button);
    }

    // Load — enabled only when a model is staged AND (a device is staged OR the
    // model is online and has none to stage), and only while this stage is
    // free. The device check is the transcription card's; without it a Load
    // whose device answer was empty or still in flight sent no device at all.
    let staged_online = shown
        .as_deref()
        .is_some_and(|m| model_is_online(backend, m));
    let staged_ok =
        selected.is_some() && (app.staged_picks.device(stage.stage()).is_some() || staged_online);
    picker_row
        .push(
            button::suggested("Load model")
                .leading_icon(icons::phosphor_handle(icons::PLAY))
                .on_press_maybe((staged_ok && app.is_model_ready(stage.stage())).then_some(
                    Message::Stage(StageMessage::Load {
                        stage: stage.stage(),
                    }),
                )),
        )
        .into()
}

/// Reload: bring the running model back up on the same device, for the case
/// the daemon cannot see — a secret edited by another tool, a model file
/// replaced underneath it. A glyph with its word in the tooltip, like the
/// header's ✕, so the pickers have room. Disabled while an operation is in
/// flight, so a second click cannot race the first.
fn reload_button(app: &AppModel, stage: BlockStage) -> Element<'_, Message> {
    super::surface::rounded_tooltip(
        widget::button::icon(icons::phosphor_handle(icons::ARROWS_CLOCKWISE))
            .class(cosmic::theme::Button::Standard)
            .on_press_maybe(app.is_model_ready(stage.stage()).then_some(Message::Stage(
                StageMessage::Reload {
                    stage: stage.stage(),
                },
            ))),
        text::body("Reload this model"),
        widget::tooltip::Position::Bottom,
    )
}

/// The model the card's controls act on: the running one, or the one its
/// pickers show.
fn card_model(app: &AppModel, stage: BlockStage, backend: &BackendInfo) -> Option<String> {
    let block = app.block(stage);
    if block.enabled {
        block.model.clone()
    } else {
        shown_model(
            app.staged_picks.for_backend(stage.stage(), &backend.source),
            block.model.as_deref(),
        )
    }
}

/// Shown when post-processors are installed but none is chosen: a prompt and
/// the button that opens the picker sheet.
///
/// The "Post-processing" heading above the section already names it, so the
/// line only has to say what a post-processor is for.
fn unselected_state<'a>(stage: BlockStage, copy: &Copy) -> Element<'a, Message> {
    prompt_row(
        copy.unselected,
        Some(
            button::suggested(copy.select)
                .leading_icon(icons::phosphor_handle(icons::PLAY))
                .on_press(Message::Shell(ShellMessage::ToggleContextPage(
                    ContextPage::SelectBlockBackend(stage),
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

/// `stage`'s picker sheet: one row per installed backend that can fill it.
/// The selected one is flagged; every other row carries a Select button.
///
/// The twin of [`select_backend_sheet`](super::select_backend_sheet), and
/// deliberately the same shape.
pub fn sheet(app: &AppModel, stage: BlockStage) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let muted = muted_text_color();
    let copy = copy(stage);
    let selected = app.block(stage).source.as_deref();

    let answered = stage_backends(app, stage);
    let eligible: &[BackendInfo] = answered.unwrap_or_default();
    let mut col = widget::column::with_capacity(eligible.len() + 1)
        .spacing(spacing.space_xs)
        .width(Length::Fill)
        .push(text::caption(copy.sheet_caption).class(cosmic::theme::Text::Color(muted)));

    if eligible.is_empty() {
        return col
            .push(text::body(if answered.is_none() {
                "Loading…"
            } else {
                copy.sheet_empty
            }))
            .into();
    }

    for backend in eligible {
        col = col.push(sheet_row(
            stage,
            backend,
            selected == Some(backend.source.as_str()),
        ));
    }
    col.into()
}

/// One backend row inside the picker sheet: glyph + name + the models it
/// serves in the stage, then either a "Selected" flag or a Select button.
fn sheet_row(
    stage: BlockStage,
    backend: &BackendInfo,
    is_selected: bool,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let muted = muted_text_color();

    // Already narrowed: the stage's backend list carries only the models that
    // stage can run, so the row shows them without filtering anything.
    let models: Vec<String> = backend.models.iter().map(|m| m.name.clone()).collect();
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
            .on_press(Message::Stage(StageMessage::SelectBackend {
                stage: stage.stage(),
                source: backend.source.clone(),
            }))
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
    //! Which backends and models a stage may offer is no longer decided here —
    //! it is `GET /stt/pipeline/2/backend/list`, and the daemon's own tests
    //! (`each_stage_lists_only_the_backends_that_can_fill_it`,
    //! `each_stage_lists_only_the_models_carrying_its_role`) cover the rule,
    //! including manifest order. What is left here is what this file still
    //! decides.
    use super::*;

    fn pick(model: &str) -> crate::state::staged_picks::StagedPick {
        crate::state::staged_picks::StagedPick {
            model: model.to_string(),
            source: "github.com/x/s1".to_string(),
            device: None,
        }
    }

    /// The regression: after an unload nothing is staged but the daemon still
    /// remembers the model, and the card shows it. The device dropdown keys off
    /// this same value, so returning `None` here is what left the card showing
    /// a model with no device picker beside it.
    #[test]
    fn the_daemons_selection_is_shown_when_nothing_is_staged() {
        assert_eq!(
            shown_model(None, Some("s1-mini-q4_k_m")),
            Some("s1-mini-q4_k_m".to_string()),
        );
    }

    /// A staged pick wins over the daemon's selection. (That the pick belongs
    /// to this backend is `StagedPicks::for_backend`'s job, tested there.)
    #[test]
    fn a_staged_pick_wins() {
        assert_eq!(
            shown_model(Some(&pick("s1-mini-q8_0")), Some("s1-mini-q4_k_m")),
            Some("s1-mini-q8_0".to_string()),
        );
    }

    /// A backend chosen with no model picked yet shows nothing, and the device
    /// picker stays hidden with it.
    #[test]
    fn nothing_is_shown_without_a_pick_or_a_selection() {
        assert_eq!(shown_model(None, None), None);
    }
}
