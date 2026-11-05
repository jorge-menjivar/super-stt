// SPDX-License-Identifier: GPL-3.0-only
//! The synthesis card's voice control: which voice the model speaks in, and
//! the one warning that says it has none.

use cosmic::Element;
use cosmic::iced::Alignment;
use cosmic::iced::widget::row;
use cosmic::widget::{self, text};

use crate::core::app::AppModel;
use crate::daemon::backends::BackendInfo;
use crate::ui::icons;
use crate::ui::messages::{Message, VoiceMessage};

/// The voice dropdown for `model`, or `None` when there is nothing to pick.
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
pub(super) fn voice_dropdown<'a>(
    backend: &'a BackendInfo,
    model: &str,
    app: &'a AppModel,
) -> Option<Element<'a, Message>> {
    // Stale-block guard, the same one the language button uses: the two values
    // below describe one model, and a card drawing another must not read them.
    if app.voice.target.as_ref() != Some(&(backend.source.clone(), model.to_string())) {
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
/// This is what a cloning model looks like before any clip has been recorded.
/// Saying it on the card turns a failure at speak time into a state the user
/// can see and fix.
///
/// A model that takes described voices is exempt: it has nothing to enumerate
/// and needs nothing chosen, so an empty list there is not a problem.
pub(super) fn voice_warning<'a>(
    backend: &'a BackendInfo,
    model: &str,
    app: &'a AppModel,
) -> Option<Element<'a, Message>> {
    if app.voice.target.as_ref() != Some(&(backend.source.clone(), model.to_string())) {
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

/// The wording [`voice_warning`] shows: which way out of "nothing to speak in"
/// this user actually has.
///
/// `has_choices` is whether the dropdown above has anything in it, `clones`
/// whether the model takes cloned ids at all. Kept free of [`AppModel`] so the
/// one thing this line must never do — send someone who has already recorded a
/// voice off to record one — is a test rather than a screenshot.
fn voice_warning_text(has_choices: bool, clones: bool) -> &'static str {
    match (has_choices, clones) {
        // Something to pick, and the dropdown offering it is directly above.
        (true, _) => "This model doesn't have built-in voices. Pick a cloned voice above.",
        (false, true) => {
            "This model doesn't have built-in voices. Record one in Text to speech, on the \
             Voices tab."
        }
        // Neither a list nor a way to fill one: a manifest that declares preset
        // voices and then ships none. Nothing to send the user to, so the line
        // states the fact and stops.
        (false, false) => "No cloned voice chosen",
    }
}

#[cfg(test)]
mod tests {
    //! Pin which way out the "nothing to speak in" line offers. The dropdown
    //! and the Voices tab are different answers, and offering the wrong one
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
    fn an_empty_library_sends_a_cloning_model_to_the_voices_tab() {
        let (text, _, records) = routes(false, true);
        assert!(records, "{text}");
        assert!(text.contains("voices tab"), "{text}");
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
