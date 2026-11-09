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
use super_voice_shared::tts::models::voices::VoiceInfo;

use super::common::error_banner;
use crate::core::app::AppModel;
use crate::state::scripts::{self, Script};
use crate::state::voices::{ScriptChoice, VoicesState};
use crate::ui::languages::friendly_name;
use crate::ui::messages::{Message, VoicesMessage};

/// The picker's first row: no script, read your own words.
fn free_row() -> String {
    crate::fl!("no-script-read-your-own-words")
}

/// The Voices tab.
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
    settings::view_column(blocks).into()
}

/// What the loaded model can do with these voices.
///
/// Stated once, at the top, rather than repeated as a disabled tooltip on
/// every row: the answer is a property of the loaded model, not of any one
/// voice.
fn capability_section(state: &VoicesState) -> Element<'_, Message> {
    let (status, detail) = match state.model.as_ref() {
        None => (
            crate::fl!("no-model-loaded"),
            crate::fl!("voices-you-add-are-kept"),
        ),
        Some(model) if !model.clones => (
            crate::fl!("model-does-not-clone", model = model.name.as_str()),
            crate::fl!("this-model-speaks-only-in-its-own-preset-voices").to_string(),
        ),
        Some(model) => {
            let budget = model.clone_ref_seconds.map_or_else(
                || crate::fl!("any-length-of-reference-audio"),
                |s| {
                    crate::fl!(
                        "up-to-seconds-of-reference-audio",
                        seconds = format!("{s:.0}")
                    )
                },
            );
            let transcript = if model.needs_transcript {
                format!(" {}", crate::fl!("it-also-needs-the-transcript"))
            } else {
                String::new()
            };
            (
                crate::fl!("model-clones-voices", model = model.name.as_str()),
                format!(
                    "{}{transcript}",
                    crate::fl!(
                        "it-uses-budget-from-each-recording",
                        budget = budget.as_str()
                    )
                ),
            )
        }
    };

    settings::section()
        .title(crate::fl!("active-model-title"))
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
        button::suggested(crate::fl!("record"))
            .on_press(Message::Voices(VoicesMessage::StartRecording)),
        button::standard(crate::fl!("import-a-wav-file"))
            .on_press(Message::Voices(VoicesMessage::ImportFile)),
    ]
    .spacing(cosmic::theme::spacing().space_xs)
    .align_y(Alignment::Center);

    let script = state.script(language);
    let mut section = settings::section()
        .title(crate::fl!("add-a-voice"))
        .add(script_picker(state, language));
    if let Some(script) = script {
        section = section.add(script_card(script, 16.0, Some(script_meta(script, state))));
    }
    section
        .add(
            settings::item::builder(crate::fl!("new-recording"))
                .description(if script.is_some() {
                    crate::fl!("press-record-and-read-the-script")
                } else {
                    crate::fl!("read-a-couple-of-sentences")
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
    labels.push(free_row());
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

    settings::item::builder(crate::fl!("script"))
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
        return crate::fl!("nothing-to-read").to_string();
    };
    match language.filter(|tag| !scripts::same_language(script, tag)) {
        // Either the catalog has no script in this language yet, or the user
        // deliberately picked another one. The sentence is true of both, and a
        // reference read in the wrong language is worth one line of caption.
        Some(tag) => crate::fl!(
            "script-language-note",
            blurb = script.blurb,
            script_language = friendly_name(script.language),
            speak_language = friendly_name(tag)
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
    let pace = crate::fl!(
        "about-seconds-at-an-unhurried-pace",
        seconds = format!("{seconds:.0}")
    );
    match state.clone_ref_seconds() {
        Some(budget) if !script.fits(Some(budget)) => crate::fl!(
            "script-longer-than-budget",
            pace = pace.as_str(),
            budget = format!("{budget:.0}")
        ),
        _ => pace,
    }
}

/// While the microphone is open: the script to read, elapsed time, the input
/// level, and Stop.
fn recording_section<'a>(state: &'a VoicesState, language: Option<&str>) -> Element<'a, Message> {
    let elapsed = state.recorder.as_ref().map_or_else(
        || {
            crate::fl!(
                "seconds-short",
                seconds = format!("{:.0}", state.recording_seconds)
            )
        },
        |r| {
            crate::fl!(
                "seconds-of-seconds",
                elapsed = format!("{:.0}", state.recording_seconds),
                max = format!("{:.0}", r.max_seconds())
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
        button::destructive(crate::fl!("stop"))
            .on_press(Message::Voices(VoicesMessage::StopRecording)),
    ]
    .spacing(cosmic::theme::spacing().space_xs)
    .align_y(Alignment::Center);

    let mut section = settings::section().title(crate::fl!("recording"));
    if let Some(script) = state.script(language) {
        section = section.add(script_card(script, 19.0, None));
    }
    section
        .add(settings::item(crate::fl!("elapsed"), text::body(elapsed)))
        .add(settings::flex_item(crate::fl!("input-level"), meter))
        .into()
}

/// A captured or imported sample, waiting to be named and saved.
fn pending_section(state: &VoicesState) -> Element<'_, Message> {
    let Some(pending) = state.pending.as_ref() else {
        // Unreachable: `add_section` checks before calling. An empty section
        // beats a panic in a view.
        return settings::section().title(crate::fl!("add-a-voice")).into();
    };

    let summary = if pending.seconds > 0.0 {
        format!("{} · {:.1} s", pending.origin.describe(), pending.seconds)
    } else {
        pending.origin.describe()
    };

    let label_input = widget::text_input(crate::fl!("name-this-voice"), &state.label_input)
        .on_input(|s| Message::Voices(VoicesMessage::LabelChanged(s)))
        .width(Length::Fill);

    let mut save = button::suggested(if state.saving {
        crate::fl!("saving-progress")
    } else {
        crate::fl!("save-voice")
    });
    if state.can_save() {
        save = save.on_press(Message::Voices(VoicesMessage::Save));
    }
    let mut discard = button::destructive(crate::fl!("discard"));
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
    let transcript = column![text::heading(crate::fl!("transcript"))]
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
            .push(text::caption(crate::fl!("from-the-built-in-script")))
            .push(script_card(script, 16.0, None)),
        None => transcript
            .push(text::caption(if state.needs_transcript() {
                crate::fl!("required-by-the-loaded-model")
            } else {
                crate::fl!("optional-some-models-clone-from-the-words")
            }))
            .push(
                widget::text_input(
                    crate::fl!("what-the-recording-says"),
                    &state.transcript_input,
                )
                .on_input(|s| Message::Voices(VoicesMessage::TranscriptChanged(s)))
                .width(Length::Fill),
            ),
    };

    settings::section()
        .title(crate::fl!("new-voice"))
        .add(settings::item(crate::fl!("sample"), text::body(summary)))
        .add(settings::flex_item(crate::fl!("name"), label_input))
        .add(transcript)
        .add(settings::flex_item("", actions))
        .into()
}

/// The stored library, one row per voice.
fn library_section(state: &VoicesState) -> Element<'_, Message> {
    let mut section = settings::section().title(crate::fl!("saved-voices"));

    if !state.loaded {
        return section
            .add(settings::item(
                "",
                text::body(crate::fl!("reading-the-library")),
            ))
            .into();
    }
    if state.voices.is_empty() {
        return section
            .add(note(
                crate::fl!("no-voices-yet"),
                crate::fl!("record-or-import-a-sample").to_string(),
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
        let field = widget::text_input(crate::fl!("name-this-voice"), label)
            .on_input(|s| Message::Voices(VoicesMessage::RenameChanged(s)))
            .on_submit(|_| Message::Voices(VoicesMessage::CommitRename))
            .width(Length::Fill);
        let actions = row![
            button::suggested(crate::fl!("save"))
                .on_press(Message::Voices(VoicesMessage::CommitRename)),
            button::standard(crate::fl!("cancel"))
                .on_press(Message::Voices(VoicesMessage::CancelRename)),
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
        let mut preview = button::standard(if busy {
            crate::fl!("preparing-progress")
        } else {
            crate::fl!("preview")
        });
        if !state.any_busy() {
            preview = preview.on_press(Message::Voices(VoicesMessage::Preview(
                voice.voice_id.clone(),
            )));
        }
        actions = actions.push(preview);
    }
    if !deleting {
        actions = actions.push(
            button::standard(crate::fl!("rename")).on_press(Message::Voices(
                VoicesMessage::BeginRename {
                    id: voice.id.clone(),
                    label: voice.label.clone(),
                },
            )),
        );
        actions = actions.push(
            button::destructive(crate::fl!("delete"))
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
        return crate::fl!("deleting-progress");
    }
    if busy {
        // Handing a clip to the model means encoding it there, which is seconds
        // rather than milliseconds the first time a model meets one. Saying so
        // is the difference between a slow page and a broken one — and it is
        // said for a preparation the daemon started as well as for a preview
        // asked for here, since the wait is the same either way.
        return crate::fl!("getting-the-model-ready");
    }
    let transcript = if voice.transcript.is_some() {
        crate::fl!("with-transcript")
    } else {
        crate::fl!("no-transcript")
    };
    crate::fl!(
        "voice-meta",
        seconds = format!("{:.1}", voice.duration_seconds),
        transcript = transcript,
        id = voice.voice_id.as_str()
    )
}
