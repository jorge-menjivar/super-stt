// SPDX-License-Identifier: GPL-3.0-only
//! Voices-page state: the library, what the loaded model can do with it, and
//! the half-finished sample the user is assembling.

use super_tts_shared::models::voices::{VoiceInfo, VoiceModelSupport};

use crate::state::scripts::{self, Script};

/// The sample waiting to be named and saved.
///
/// Recording and importing converge here, so everything after the sample is
/// obtained — naming it, adding a transcript, saving it — is one path with one
/// set of validation rules.
#[derive(Debug, Clone)]
pub struct PendingSample {
    /// A complete WAV file, ready to upload.
    pub wav: Vec<u8>,
    /// Length in seconds, for the "18.4 s captured" readout.
    pub seconds: f32,
    /// Where it came from, which is the only thing the two paths still differ
    /// on once the bytes exist.
    pub origin: SampleOrigin,
    /// The script this clip was read from, when it was recorded against one.
    ///
    /// Held on the sample rather than read back off the picker, because it is a
    /// fact about the recording that already happened: changing the picker
    /// afterwards cannot change which words were spoken. It is also what makes
    /// the transcript read-only — a built-in script's wording is exact, and an
    /// edit to it describes audio that does not exist.
    pub script: Option<&'static Script>,
}

/// How a pending sample was obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SampleOrigin {
    /// Captured from the microphone in this window.
    Recorded,
    /// Imported from a file, named here so the user can tell two samples apart
    /// before either is saved.
    Imported(String),
}

impl SampleOrigin {
    /// A short label for the pending-sample card.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Recorded => "Recorded just now".to_string(),
            Self::Imported(name) => format!("Imported from {name}"),
        }
    }
}

/// Which reading script the user is recording against.
///
/// Three states rather than an `Option`, because "not chosen yet" and "chose to
/// read my own words" must not look alike: the first follows the speech
/// language as the daemon reports it, and the second has to survive a model
/// switch that changes that language. An `Option` would have to be seeded the
/// moment the page opened — before the daemon has said which language it
/// speaks — and would then either stick at a stale default or overwrite a
/// deliberate choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScriptChoice {
    /// Nothing picked: the page offers the best default for the speech
    /// language, recomputed each time it renders.
    #[default]
    Suggested,
    /// Read your own words. No script is shown and no transcript is prefilled.
    Free,
    /// One script from the catalog, picked by the user.
    Chosen(&'static Script),
}

/// Everything the Voices page renders and edits.
#[derive(Debug, Default)]
pub struct VoicesState {
    /// The library, newest first.
    pub voices: Vec<VoiceInfo>,
    /// What the loaded model can do with a cloned voice; `None` while nothing
    /// is loaded.
    pub model: Option<VoiceModelSupport>,
    /// True until the first listing arrives, so an empty library and a library
    /// that has not been read yet do not look the same.
    pub loaded: bool,
    /// The script the user means to read, if any.
    pub script: ScriptChoice,
    /// A capture in progress. Its presence *is* the recording state — there is
    /// no separate flag to keep in step with it.
    pub recorder: Option<crate::audio::recorder::Recorder>,
    /// Seconds captured so far, refreshed by the recording tick.
    pub recording_seconds: f32,
    /// Peak input level, `0.0`–`1.0`, for the meter.
    pub recording_level: f32,
    /// The sample waiting to be saved.
    pub pending: Option<PendingSample>,
    /// Label field for the pending sample.
    pub label_input: String,
    /// Transcript field for the pending sample.
    pub transcript_input: String,
    /// The upload is in flight; the Save button is spinning.
    pub saving: bool,
    /// Voice id currently being renamed, and the text being typed.
    pub renaming: Option<(String, String)>,
    /// Voice ids with a delete in flight, so the row can grey out.
    pub deleting: Vec<String>,
    /// The `voice_id` the daemon says it is preparing, from `/events`.
    ///
    /// Distinct from [`Self::previewing`], which is this window's own request:
    /// a preparation starts when *any* client saves a clip or asks to speak in
    /// a voice the loaded model has not been given yet, and the row should say
    /// so whoever set it off. Saving a voice here is the common case — the
    /// daemon starts preparing it immediately, and this is what shows that.
    pub preparing: Option<String>,
    /// The `voice_id` of a preview that has been asked for and not yet come
    /// back.
    ///
    /// A preview is not instant: the first one after a model loads pushes the
    /// whole reference clip into the backend, which encodes it, and that can
    /// run for the better part of a minute. The daemon cancels whatever is
    /// playing when a new utterance arrives, so a second press does not queue
    /// a second preview — it kills the first. This is what lets the page say
    /// so, and stop the press from being possible.
    pub previewing: Option<String>,
}

impl VoicesState {
    /// Whether this voice is being got ready to speak — either a preview this
    /// window asked for, or a preparation the daemon announced.
    #[must_use]
    pub fn is_busy(&self, voice_id: &str) -> bool {
        self.previewing.as_deref() == Some(voice_id) || self.preparing.as_deref() == Some(voice_id)
    }

    /// Whether anything is being got ready, by anyone.
    #[must_use]
    pub fn any_busy(&self) -> bool {
        self.previewing.is_some() || self.preparing.is_some()
    }

    /// Whether a capture is running right now.
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Whether the loaded model can speak in a cloned voice.
    #[must_use]
    pub fn model_clones(&self) -> bool {
        self.model.as_ref().is_some_and(|m| m.clones)
    }

    /// Whether the loaded model needs a transcript alongside the audio, which
    /// is what turns the transcript field from optional into required.
    #[must_use]
    pub fn needs_transcript(&self) -> bool {
        self.model.as_ref().is_some_and(|m| m.needs_transcript)
    }

    /// The script to display and to prefill the transcript from, resolved
    /// against the speech language the daemon reports.
    ///
    /// `preferred` is the global Primary Language: the loaded model's own
    /// capability block does not carry a language, and the clip should be read
    /// in the language the voice will be asked to speak. The budget it *does*
    /// carry narrows the suggestion to a script the model can hear all of.
    #[must_use]
    pub fn script(&self, preferred: Option<&str>) -> Option<&'static Script> {
        match self.script {
            ScriptChoice::Suggested => scripts::default_for(preferred, self.clone_ref_seconds()),
            ScriptChoice::Free => None,
            ScriptChoice::Chosen(script) => Some(script),
        }
    }

    /// How much reference audio the loaded model takes, when it says.
    #[must_use]
    pub fn clone_ref_seconds(&self) -> Option<f32> {
        self.model.as_ref().and_then(|m| m.clone_ref_seconds)
    }

    /// Whether the pending sample can be saved as it stands.
    ///
    /// The transcript counts only when the loaded model needs one. The library
    /// outlives the loaded model, so a transcript-less voice is perfectly
    /// valid — it just cannot be handed to *this* model, and the button is
    /// where that is said rather than after an upload the daemon refuses.
    #[must_use]
    pub fn can_save(&self) -> bool {
        self.pending.is_some()
            && !self.saving
            && !self.label_input.trim().is_empty()
            && (!self.needs_transcript() || !self.transcript_input.trim().is_empty())
    }

    /// Drop the pending sample and the fields that describe it.
    ///
    /// The script choice survives: it describes the *next* recording as much as
    /// the one being thrown away, and a user who discards a bad take almost
    /// always means to read the same words again.
    pub fn clear_pending(&mut self) {
        self.pending = None;
        self.label_input.clear();
        self.transcript_input.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{ScriptChoice, VoicesState};
    use crate::state::scripts;

    #[test]
    fn an_untouched_page_suggests_a_script_in_the_speech_language() {
        let state = VoicesState::default();
        let suggested = state.script(Some("es-MX")).expect("Spanish is catalogued");
        assert_eq!(suggested.language, "es");
        // And it keeps following the language rather than latching onto the
        // first one it was asked about.
        assert_eq!(state.script(Some("ja")).map(|s| s.language), Some("ja"));
    }

    #[test]
    fn choosing_free_survives_a_language_change() {
        let state = VoicesState {
            script: ScriptChoice::Free,
            ..VoicesState::default()
        };
        assert!(state.script(Some("es")).is_none());
        assert!(state.script(None).is_none());
    }

    #[test]
    fn a_chosen_script_outranks_the_suggestion() {
        let chosen = scripts::find("en-north-wind").expect("catalogued");
        let state = VoicesState {
            script: ScriptChoice::Chosen(chosen),
            ..VoicesState::default()
        };
        // Even when the speech language would have suggested another one.
        assert_eq!(state.script(Some("fr")), Some(chosen));
    }
}
