// SPDX-License-Identifier: GPL-3.0-only
//! The Voices page: the cloned-voice library, and the recording or import that
//! adds to it.
//!
//! The page is readable and editable whatever model is loaded. A voice is
//! stored independently of any model — the same recording is handed to
//! whichever model is loaded next — so gating the library on the active model
//! would hide the user's own recordings behind a setting they may be about to
//! change. What the loaded model *can do* with them is stated at the top
//! instead, and it is the only thing that greys out a control.

use cosmic::Element;
use cosmic::iced::widget::text::LineHeight;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, settings, text};
use super_tts_shared::models::voices::VoiceInfo;

use super::common::{error_banner, page_layout};
use crate::core::app::AppModel;
use crate::state::scripts::{self, Script};
use crate::state::voices::{ScriptChoice, VoicesState};
use crate::ui::languages::friendly_name;
use crate::ui::messages::{Message, VoicesMessage};

/// The picker's first row: no script, read your own words.
const FREE_ROW: &str = "No script — read your own words";

/// Voices page.
pub fn page(app: &AppModel) -> Element<'_, Message> {
    let state = &app.voices;
    // The global Primary Language decides which script is suggested: a clone is
    // best conditioned on speech in the language it will be asked to speak, and
    // the loaded model's capability block does not carry one.
    let language = app.language.primary_language.as_deref();
    let mut blocks = Vec::new();
    if let Some(message) = app.action_error_for(crate::state::ErrorScope::Voices) {
        blocks.push(error_banner(message));
    }
    blocks.push(capability_section(state));
    blocks.push(add_section(state, language));
    blocks.push(library_section(state));
    page_layout("Voices", settings::view_column(blocks))
}

/// What the loaded model can do with these voices.
///
/// Stated once, at the top, rather than repeated as a disabled tooltip on
/// every row: the answer is a property of the loaded model, not of any one
/// voice.
fn capability_section(state: &VoicesState) -> Element<'_, Message> {
    let (status, detail) = match state.model.as_ref() {
        None => (
            "No model loaded".to_string(),
            "Voices you add are kept. Load a model that clones to speak in them.".to_string(),
        ),
        Some(model) if !model.clones => (
            format!("{} does not clone", model.name),
            "This model speaks only in its own preset voices. Your saved voices are untouched \
             and become available when a cloning model is loaded."
                .to_string(),
        ),
        Some(model) => {
            let budget = model.clone_ref_seconds.map_or_else(
                || "any length of reference audio".to_string(),
                |s| format!("up to {s:.0} seconds of reference audio"),
            );
            let transcript = if model.needs_transcript {
                " It also needs the transcript of what the recording says."
            } else {
                ""
            };
            (
                format!("{} clones voices", model.name),
                format!("It uses {budget} from each recording.{transcript}"),
            )
        }
    };

    settings::section()
        .title("Active model")
        .add(note(status, detail))
        .into()
}

/// A statement plus its explanation, as a section row.
///
/// `settings::item::builder` needs a control to become a widget, and these
/// rows have nothing to operate — they are the page telling the user where
/// they stand.
fn note(heading: String, detail: String) -> Element<'static, Message> {
    column![text::heading(heading), text::caption(detail)]
        .spacing(cosmic::theme::spacing().space_xxxs)
        .width(Length::Fill)
        .into()
}

/// The add-a-voice section: recording, the pending sample, or the two ways to
/// start.
fn add_section<'a>(state: &'a VoicesState, language: Option<&str>) -> Element<'a, Message> {
    if state.is_recording() {
        return recording_section(state, language);
    }
    if state.pending.is_some() {
        return pending_section(state);
    }

    let controls = row![
        button::suggested("Record").on_press(Message::Voices(VoicesMessage::StartRecording)),
        button::standard("Import a WAV file…").on_press(Message::Voices(VoicesMessage::ImportFile)),
    ]
    .spacing(cosmic::theme::spacing().space_xs)
    .align_y(Alignment::Center);

    let script = state.script(language);
    let mut section = settings::section()
        .title("Add a voice")
        .add(script_picker(state, language));
    if let Some(script) = script {
        section = section.add(script_card(script, 16.0, Some(script_meta(script, state))));
    }
    section
        .add(
            settings::item::builder("New recording")
                .description(if script.is_some() {
                    "Press Record and read the script above at your normal speaking pace, \
                     somewhere quiet. Recording stops on its own after two minutes."
                } else {
                    "Read a couple of sentences in a normal speaking voice, somewhere quiet. \
                     Recording stops on its own after two minutes."
                })
                .control(controls),
        )
        .into()
}

/// The script picker: what to read while the microphone is open.
///
/// Ordered by the speech language and never filtered by it, so a user whose
/// language this catalog has no script for still sees the rest. Row zero is
/// always "no script", because improvising was the only way to record a voice
/// before this picker existed and it is still a perfectly good one.
fn script_picker(state: &VoicesState, language: Option<&str>) -> Element<'static, Message> {
    let offered = scripts::offered(language);
    let mut labels: Vec<String> = Vec::with_capacity(offered.len() + 1);
    labels.push(FREE_ROW.to_string());
    labels.extend(
        offered
            .iter()
            .map(|s| format!("{} · {}", friendly_name(s.language), s.title)),
    );
    // Row → script id, so the message carries what was picked rather than where
    // it happened to sit in a list this function rebuilds on every render.
    let ids: Vec<Option<&'static str>> = std::iter::once(None)
        .chain(offered.iter().map(|s| Some(s.id)))
        .collect();

    let chosen = state.script(language);
    let selected = match state.script {
        ScriptChoice::Free => Some(0),
        _ => chosen.and_then(|script| ids.iter().position(|id| *id == Some(script.id))),
    };

    settings::item::builder("Script")
        .description(picker_description(chosen, language))
        .control(widget::dropdown(labels, selected, move |index| {
            Message::Voices(VoicesMessage::ScriptSelected(
                ids.get(index).copied().flatten(),
            ))
        }))
        .into()
}

/// The line under the picker: what the chosen script is for, and whether it is
/// in the language the voice will be asked to speak.
fn picker_description(chosen: Option<&'static Script>, language: Option<&str>) -> String {
    let Some(script) = chosen else {
        return "Nothing to read. Record in your own words, then type them in afterwards."
            .to_string();
    };
    match language.filter(|tag| !scripts::same_language(script, tag)) {
        // Either the catalog has no script in this language yet, or the user
        // deliberately picked another one. The sentence is true of both, and a
        // reference read in the wrong language is worth one line of caption.
        Some(tag) => format!(
            "{} Note that this script is in {}, and the daemon is set to speak {} — a clone \
             matches best when the reference is read in the language it will speak.",
            script.blurb,
            friendly_name(script.language),
            friendly_name(tag),
        ),
        None => script.blurb.to_string(),
    }
}

/// The script itself, as a block: to read from before and during a recording,
/// and to read back as the transcript afterwards.
///
/// Added to its section directly, never through `settings::flex_item`: that
/// helper wraps a control in a `Length::Shrink` container, and a block of
/// `Length::Fill` text inside one collapses to nothing — which is exactly what
/// this card did while recording, where the text is its only child and there is
/// no caption underneath to give the column an intrinsic width to shrink to.
///
/// `size` is larger while the microphone is open: that is the one moment this
/// page is read from a chair's distance rather than looked at. `meta` is the
/// line underneath, and is left off in the states where it would only be one
/// more thing to read aloud by mistake.
fn script_card(
    script: &'static Script,
    size: f32,
    meta: Option<String>,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let body = widget::text(script.text)
        .size(size)
        .line_height(LineHeight::Relative(1.45))
        .width(Length::Fill);

    let mut card = column![body].spacing(spacing.space_xxs).width(Length::Fill);
    if let Some(meta) = meta {
        card = card.push(text::caption(meta));
    }

    widget::container(card)
        .padding(spacing.space_s)
        .width(Length::Fill)
        .class(cosmic::theme::Container::custom(move |theme| {
            let cosmic = theme.cosmic();
            let component = &theme.current_container().component;
            cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(component.base.into())),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_s.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .into()
}

/// How long a script takes to read, and a warning when the loaded model cannot
/// hear all of it.
///
/// Overrunning the budget is not a matter of wasted breath: the daemon trims
/// the clip to the budget and stores the transcript whole, so the model is
/// handed audio that stops mid-sentence against words that carry on. Saying
/// "read on anyway" here, as this line first did, was advice that quietly
/// produced worse clones.
fn script_meta(script: &Script, state: &VoicesState) -> String {
    let seconds = script.estimated_seconds();
    let pace = format!("About {seconds:.0} seconds at an unhurried pace.");
    match state.clone_ref_seconds() {
        Some(budget) if !script.fits(Some(budget)) => format!(
            "{pace} That is longer than the {budget:.0} s this model takes, and it would be \
             given the words it never hears — pick a shorter script, or stop reading at \
             {budget:.0} seconds."
        ),
        _ => pace,
    }
}

/// While the microphone is open: the script to read, elapsed time, the input
/// level, and Stop.
fn recording_section<'a>(state: &'a VoicesState, language: Option<&str>) -> Element<'a, Message> {
    let elapsed = state.recorder.as_ref().map_or_else(
        || format!("{:.0} s", state.recording_seconds),
        |r| {
            format!(
                "{:.0} s of {:.0} s",
                state.recording_seconds,
                r.max_seconds()
            )
        },
    );
    // A floor under any non-zero level so a quiet room still shows the meter
    // moving — an input that reads as flat zero looks like a dead microphone,
    // which is the one thing this control exists to disprove.
    let level =
        widget::determinate_linear(state.recording_level.max(if state.recording_level > 0.0 {
            0.05
        } else {
            0.0
        }))
        // A fixed width, not `Length::Fill`: the bar lays out with
        // `layout::atomic`, and `settings::flex_item` measures its control
        // inside a `Length::Shrink` container, where a fill width has nothing
        // to fill and the bar vanishes — which is what this meter had been
        // doing, in the one state where it is the only proof the microphone is
        // alive.
        .width(Length::Fixed(320.0));
    let meter = row![
        level,
        button::destructive("Stop").on_press(Message::Voices(VoicesMessage::StopRecording)),
    ]
    .spacing(cosmic::theme::spacing().space_xs)
    .align_y(Alignment::Center);

    let mut section = settings::section().title("Recording");
    if let Some(script) = state.script(language) {
        section = section.add(script_card(script, 19.0, None));
    }
    section
        .add(settings::item("Elapsed", text::body(elapsed)))
        .add(settings::flex_item("Input level", meter))
        .into()
}

/// A captured or imported sample, waiting to be named and saved.
fn pending_section(state: &VoicesState) -> Element<'_, Message> {
    let Some(pending) = state.pending.as_ref() else {
        // Unreachable: `add_section` checks before calling. An empty section
        // beats a panic in a view.
        return settings::section().title("Add a voice").into();
    };

    let summary = if pending.seconds > 0.0 {
        format!("{} · {:.1} s", pending.origin.describe(), pending.seconds)
    } else {
        pending.origin.describe()
    };

    let label_input = widget::text_input("Name this voice", &state.label_input)
        .on_input(|s| Message::Voices(VoicesMessage::LabelChanged(s)))
        .width(Length::Fill);

    let mut save = button::suggested(if state.saving {
        "Saving…"
    } else {
        "Save voice"
    });
    if state.can_save() {
        save = save.on_press(Message::Voices(VoicesMessage::Save));
    }
    let mut discard = button::destructive("Discard");
    if !state.saving {
        discard = discard.on_press(Message::Voices(VoicesMessage::DiscardPending));
    }
    let actions = row![save, discard]
        .spacing(cosmic::theme::spacing().space_xs)
        .align_y(Alignment::Center);

    // The transcript gets a row of its own rather than a control squeezed in
    // beside its own description: it holds a whole script, and a field showing
    // the first four words of it is a field the user cannot check. Laid out
    // like `note`, which is the section's full width.
    let spacing = cosmic::theme::spacing();
    let transcript = column![text::heading("Transcript")]
        .spacing(spacing.space_xxs)
        .width(Length::Fill);
    // A built-in script is shown, not offered for editing. Its wording is the
    // wording that was read, and the only edits possible to it are wrong ones:
    // a typo here describes audio that does not exist, and models that clone
    // in-context align the reference to these exact words. Recording without a
    // script is how you get a transcript of your own, and the caption says so
    // rather than leaving the locked field unexplained.
    let transcript = match pending.script {
        Some(script) => transcript
            .push(text::caption(
                "From the built-in script you read, and saved exactly as written. To write \
                 your own words, discard this and record again with “No script”.",
            ))
            .push(script_card(script, 16.0, None)),
        None => transcript
            .push(text::caption(if state.needs_transcript() {
                "Required by the loaded model: type exactly what the recording says."
            } else {
                "Optional. Some models clone from the words as well as the sound — \
                 adding it now means the voice works with those too."
            }))
            .push(
                widget::text_input("What the recording says", &state.transcript_input)
                    .on_input(|s| Message::Voices(VoicesMessage::TranscriptChanged(s)))
                    .width(Length::Fill),
            ),
    };

    settings::section()
        .title("New voice")
        .add(settings::item("Sample", text::body(summary)))
        .add(settings::flex_item("Name", label_input))
        .add(transcript)
        .add(settings::flex_item("", actions))
        .into()
}

/// The stored library, one row per voice.
fn library_section(state: &VoicesState) -> Element<'_, Message> {
    let mut section = settings::section().title("Saved voices");

    if !state.loaded {
        return section
            .add(settings::item("", text::body("Reading the library…")))
            .into();
    }
    if state.voices.is_empty() {
        return section
            .add(note(
                "No voices yet".to_string(),
                "Record or import a sample above, and it appears here with the id you pass \
                 to any app that speaks through Super TTS."
                    .to_string(),
            ))
            .into();
    }

    for voice in &state.voices {
        section = section.add(voice_row(voice, state));
    }
    section.into()
}

/// One saved voice: its facts, and what can be done to it.
fn voice_row<'a>(voice: &'a VoiceInfo, state: &'a VoicesState) -> Element<'a, Message> {
    let renaming = state
        .renaming
        .as_ref()
        .filter(|(id, _)| id == &voice.id)
        .map(|(_, label)| label);
    let deleting = state.deleting.iter().any(|id| id == &voice.id);
    let spacing = cosmic::theme::spacing();

    if let Some(label) = renaming {
        let field = widget::text_input("Name this voice", label)
            .on_input(|s| Message::Voices(VoicesMessage::RenameChanged(s)))
            .on_submit(|_| Message::Voices(VoicesMessage::CommitRename))
            .width(Length::Fill);
        let actions = row![
            button::suggested("Save").on_press(Message::Voices(VoicesMessage::CommitRename)),
            button::standard("Cancel").on_press(Message::Voices(VoicesMessage::CancelRename)),
        ]
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center);
        return settings::flex_item(
            "",
            column![field, actions]
                .spacing(spacing.space_xs)
                .width(Length::Fill),
        )
        .into();
    }

    let mut actions = widget::row::with_capacity(3)
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center);
    if state.model_clones() {
        // Only offered when the loaded model can actually speak it; the
        // alternative is a button whose every press is an error banner.
        //
        // Live while nothing is being previewed, and only then: the daemon
        // cancels what is playing when a new utterance arrives, so a second
        // press during the wait would end the preview it looks like it is
        // asking for. The wait is real — a first preview registers the whole
        // reference clip with the backend, which encodes it — so the button
        // says what it is doing rather than leaving a dead-looking page to
        // invite that press.
        let busy = state.is_busy(&voice.voice_id);
        let mut preview = button::standard(if busy { "Preparing…" } else { "Preview" });
        if !state.any_busy() {
            preview = preview.on_press(Message::Voices(VoicesMessage::Preview(
                voice.voice_id.clone(),
            )));
        }
        actions = actions.push(preview);
    }
    if !deleting {
        actions = actions.push(button::standard("Rename").on_press(Message::Voices(
            VoicesMessage::BeginRename {
                id: voice.id.clone(),
                label: voice.label.clone(),
            },
        )));
        actions = actions.push(
            button::destructive("Delete")
                .on_press(Message::Voices(VoicesMessage::Delete(voice.id.clone()))),
        );
    }

    settings::item::builder(voice.label.clone())
        .description(describe(voice, deleting, state.is_busy(&voice.voice_id)))
        .control(actions)
        .into()
}

/// The facts under a voice's name: how long the recording is, whether it
/// carries a transcript, and the id an app would use.
fn describe(voice: &VoiceInfo, deleting: bool, busy: bool) -> String {
    if deleting {
        return "Deleting…".to_string();
    }
    if busy {
        // Handing a clip to the model means encoding it there, which is seconds
        // rather than milliseconds the first time a model meets one. Saying so
        // is the difference between a slow page and a broken one — and it is
        // said for a preparation the daemon started as well as for a preview
        // asked for here, since the wait is the same either way.
        return "Getting the model ready to speak in this voice…".to_string();
    }
    let transcript = if voice.transcript.is_some() {
        "with transcript"
    } else {
        "no transcript"
    };
    format!(
        "{:.1} s · {transcript} · {}",
        voice.duration_seconds, voice.voice_id
    )
}
