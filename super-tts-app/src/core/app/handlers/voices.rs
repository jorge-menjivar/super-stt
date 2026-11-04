// SPDX-License-Identifier: GPL-3.0-only
//! Voices-page handling: the library, and the recording or import that fills
//! it.
//!
//! Recording and importing converge on one pending sample as soon as the bytes
//! exist, so naming, validating, and uploading are written once. The recorder
//! itself never travels in a message — it owns a device and a thread, and both
//! belong to the page for exactly as long as the user is holding the button.

use crate::audio::recorder::Recorder;
use crate::core::app::AppModel;
use crate::daemon::client::{
    create_voice, delete_voice, list_voices, rename_voice, speak_in_voice,
};
use crate::state::ErrorScope;
use crate::state::scripts;
use crate::state::voices::{PendingSample, SampleOrigin, ScriptChoice};
use crate::ui::messages::{Message, VoicesMessage};
use cosmic::prelude::*;
use super_tts_shared::models::protocol::SYNTHESIS_STAGE;

/// The sentence a preview speaks. Fixed rather than user-supplied: the point
/// is to hear the voice, and a field to fill in first is friction between the
/// user and the only question they are asking.
const PREVIEW_TEXT: &str = "This is how the cloned voice sounds when it reads a sentence aloud.";

/// Longest clip the daemon's library will store, mirrored here so the recorder
/// stops itself at the same place rather than recording audio that would be
/// refused on upload. Keep in step with `super-tts-daemon`'s
/// `voices::MAX_CLIP_SECONDS`.
const MAX_CLIP_SECONDS: f32 = 120.0;

impl AppModel {
    /// Route Voices-page messages.
    ///
    /// Split the way [`handle_speech_messages`](Self::handle_speech_messages)
    /// is: the library half (what is stored) and the capture half (what is
    /// being added) share only the page, and each `match` stays exhaustive
    /// over its own group.
    pub(in crate::core::app) fn handle_voices_messages(
        &mut self,
        message: VoicesMessage,
    ) -> Task<cosmic::Action<Message>> {
        match &message {
            // Every failure whose whole handling is "say so on the page". They
            // are one arm because the difference between them is the sentence,
            // not the behavior — the failures that also have state to unwind
            // (`SaveFailed`, `DeleteFailed`) are handled in their own groups.
            VoicesMessage::LoadFailed(_)
            | VoicesMessage::ImportFailed(_)
            | VoicesMessage::RenameFailed(_)
            | VoicesMessage::PreviewFailed(_) => {
                let (VoicesMessage::LoadFailed(m)
                | VoicesMessage::ImportFailed(m)
                | VoicesMessage::RenameFailed(m)
                | VoicesMessage::PreviewFailed(m)) = message
                else {
                    unreachable!("the outer match admitted only these four")
                };
                log::warn!("voices: {m}");
                // Only `PreviewFailed` can have one in flight, and clearing it
                // unconditionally costs nothing: the other three never set it.
                self.voices.previewing = None;
                self.set_action_error(ErrorScope::Voices, m);
                Task::none()
            }

            VoicesMessage::Refresh
            | VoicesMessage::VoicesLoaded { .. }
            | VoicesMessage::BeginRename { .. }
            | VoicesMessage::RenameChanged(_)
            | VoicesMessage::CommitRename
            | VoicesMessage::CancelRename
            | VoicesMessage::Renamed(_)
            | VoicesMessage::Delete(_)
            | VoicesMessage::Deleted(_)
            | VoicesMessage::DeleteFailed { .. }
            | VoicesMessage::Preview(_)
            | VoicesMessage::Previewed => self.handle_voice_library(message),

            VoicesMessage::ScriptSelected(_)
            | VoicesMessage::StartRecording
            | VoicesMessage::RecordingTick
            | VoicesMessage::StopRecording
            | VoicesMessage::ImportFile
            | VoicesMessage::FileImported(_)
            | VoicesMessage::LabelChanged(_)
            | VoicesMessage::TranscriptChanged(_)
            | VoicesMessage::DiscardPending
            | VoicesMessage::Save
            | VoicesMessage::Saved(_)
            | VoicesMessage::SaveFailed(_) => self.handle_voice_capture(message),
        }
    }

    /// The stored library: reading it, renaming, deleting, previewing.
    fn handle_voice_library(&mut self, message: VoicesMessage) -> Task<cosmic::Action<Message>> {
        match message {
            VoicesMessage::Refresh => refresh_library(),

            VoicesMessage::VoicesLoaded { voices, model } => {
                self.voices.voices = voices;
                self.voices.model = model;
                self.voices.loaded = true;
                self.clear_action_error(ErrorScope::Voices);
                Task::none()
            }

            VoicesMessage::BeginRename { id, label } => {
                self.voices.renaming = Some((id, label));
                Task::none()
            }

            VoicesMessage::RenameChanged(label) => {
                if let Some((_, current)) = self.voices.renaming.as_mut() {
                    *current = label;
                }
                Task::none()
            }

            VoicesMessage::CancelRename => {
                self.voices.renaming = None;
                Task::none()
            }

            VoicesMessage::CommitRename => self.commit_rename(),

            VoicesMessage::Renamed(voice) => {
                if let Some(existing) = self.voices.voices.iter_mut().find(|v| v.id == voice.id) {
                    *existing = voice;
                }
                // The Models card lists this voice under its old name until
                // something re-reads it.
                self.reload_model_voices()
            }

            VoicesMessage::Delete(id) => self.delete(id),

            VoicesMessage::Deleted(id) => {
                self.voices.deleting.retain(|d| d != &id);
                self.voices.voices.retain(|v| v.id != id);
                // Otherwise the Models card keeps offering a voice that no
                // longer exists, and picking it is an error the user cannot
                // have anticipated.
                self.reload_model_voices()
            }

            VoicesMessage::DeleteFailed { id, message } => {
                self.voices.deleting.retain(|d| d != &id);
                self.set_action_error(ErrorScope::Voices, message);
                Task::none()
            }

            VoicesMessage::Preview(voice_id) => self.preview(voice_id),

            VoicesMessage::Previewed => {
                self.voices.previewing = None;
                Task::none()
            }

            // Handled in the router or by the capture half. Listed rather
            // than caught by a wildcard: the enum's whole point is that a new
            // variant is a compile error at every end, and `_ =>` would make
            // one a silent no-op here.
            other @ (VoicesMessage::ScriptSelected(_)
            | VoicesMessage::StartRecording
            | VoicesMessage::RecordingTick
            | VoicesMessage::StopRecording
            | VoicesMessage::ImportFile
            | VoicesMessage::FileImported(_)
            | VoicesMessage::LabelChanged(_)
            | VoicesMessage::TranscriptChanged(_)
            | VoicesMessage::DiscardPending
            | VoicesMessage::Save
            | VoicesMessage::Saved(_)
            | VoicesMessage::SaveFailed(_)
            | VoicesMessage::LoadFailed(_)
            | VoicesMessage::ImportFailed(_)
            | VoicesMessage::RenameFailed(_)
            | VoicesMessage::PreviewFailed(_)) => {
                log::error!("voices: the library handler cannot serve {other:?}");
                Task::none()
            }
        }
    }

    /// Adding a voice: the microphone, the file picker, and the pending sample
    /// both of them produce.
    fn handle_voice_capture(&mut self, message: VoicesMessage) -> Task<cosmic::Action<Message>> {
        match message {
            VoicesMessage::ScriptSelected(id) => {
                // An id that no longer names a script lands on `Free`, which is
                // also where the picker's own "read your own words" row lands:
                // both mean there is nothing to put on screen, and neither is
                // worth an error banner.
                self.voices.script = match id.and_then(scripts::find) {
                    Some(script) => ScriptChoice::Chosen(script),
                    None => ScriptChoice::Free,
                };
                Task::none()
            }

            VoicesMessage::StartRecording => self.start_recording(),
            VoicesMessage::RecordingTick => self.recording_tick(),
            VoicesMessage::StopRecording => self.stop_recording(),

            VoicesMessage::ImportFile => {
                self.clear_action_error(ErrorScope::Voices);
                Task::perform(pick_wav(), |picked| {
                    cosmic::Action::App(Message::Voices(match picked {
                        Ok(picked) => VoicesMessage::FileImported(picked),
                        Err(e) => VoicesMessage::ImportFailed(e),
                    }))
                })
            }

            VoicesMessage::FileImported(picked) => {
                let Some((name, wav)) = picked else {
                    // Cancelled the picker; nothing to say about it.
                    return Task::none();
                };
                let seconds = wav_seconds(&wav);
                if self.voices.label_input.trim().is_empty() {
                    // Seed the label from the filename — it is nearly always
                    // what the user would have typed, and it is one field
                    // fewer between them and a saved voice.
                    self.voices.label_input = label_from_filename(&name);
                }
                self.voices.pending = Some(PendingSample {
                    wav,
                    seconds,
                    origin: SampleOrigin::Imported(name),
                    // Never script-backed: whatever the picker says, nobody
                    // knows what words are in a file chosen off disk.
                    script: None,
                });
                Task::none()
            }

            VoicesMessage::LabelChanged(label) => {
                self.voices.label_input = label;
                Task::none()
            }

            VoicesMessage::TranscriptChanged(transcript) => {
                // Dropped for a clip read from a built-in script: those words
                // are a fact about the audio, not a field to type over, and the
                // pending card offers no input for them. Enforced here as well
                // as in the view so the rule survives the next edit to either.
                if self
                    .voices
                    .pending
                    .as_ref()
                    .is_none_or(|pending| pending.script.is_none())
                {
                    self.voices.transcript_input = transcript;
                }
                Task::none()
            }

            VoicesMessage::DiscardPending => {
                self.voices.clear_pending();
                self.clear_action_error(ErrorScope::Voices);
                Task::none()
            }

            VoicesMessage::Save => self.save_pending(),

            VoicesMessage::Saved(voice) => {
                log::info!("stored cloned voice {} ({})", voice.label, voice.voice_id);
                self.voices.saving = false;
                self.voices.clear_pending();
                self.clear_action_error(ErrorScope::Voices);
                // Re-listing rather than inserting the new voice locally: the
                // daemon owns the ordering, and one round trip is cheaper than
                // a second sort that has to agree with it.
                Task::batch([
                    self.dispatch(Message::Voices(VoicesMessage::Refresh)),
                    self.reload_model_voices(),
                ])
            }

            VoicesMessage::SaveFailed(message) => {
                self.voices.saving = false;
                self.set_action_error(ErrorScope::Voices, message);
                Task::none()
            }

            // Handled in the router or by the library half; see the note on
            // the matching arm there.
            other @ (VoicesMessage::Refresh
            | VoicesMessage::VoicesLoaded { .. }
            | VoicesMessage::BeginRename { .. }
            | VoicesMessage::RenameChanged(_)
            | VoicesMessage::CommitRename
            | VoicesMessage::CancelRename
            | VoicesMessage::Renamed(_)
            | VoicesMessage::Delete(_)
            | VoicesMessage::Deleted(_)
            | VoicesMessage::DeleteFailed { .. }
            | VoicesMessage::Preview(_)
            | VoicesMessage::Previewed
            | VoicesMessage::LoadFailed(_)
            | VoicesMessage::ImportFailed(_)
            | VoicesMessage::RenameFailed(_)
            | VoicesMessage::PreviewFailed(_)) => {
                log::error!("voices: the capture handler cannot serve {other:?}");
                Task::none()
            }
        }
    }

    /// Re-read the loaded model's voice list, because the library it draws on
    /// just changed.
    ///
    /// The Models card offers the voices a model can be pinned to, and that
    /// list is the daemon's join of the model's own presets with the cloned
    /// library. It was read when a model became selected, staged or loaded, and
    /// at no other time — so a voice recorded here did not appear there until
    /// the app was restarted, and a deleted one was still offered.
    ///
    /// Addressed to the pair the card is showing rather than to "the active
    /// model", for the same reason every other write on that card is: a reply
    /// that lands after a model switch must not be applied to the model the
    /// user moved to.
    fn reload_model_voices(&self) -> Task<cosmic::Action<Message>> {
        let Some((source, model)) = self.voice.target.clone() else {
            return Task::none();
        };
        self.load_model_voice(&source, model)
    }

    /// Send the pending rename, if the field holds a usable name.
    ///
    /// An empty field is treated as a cancel rather than sent: the daemon
    /// answers `400` for an empty label, and a round trip that can only fail
    /// is worse than doing nothing.
    fn commit_rename(&mut self) -> Task<cosmic::Action<Message>> {
        let Some((id, label)) = self.voices.renaming.take() else {
            return Task::none();
        };
        let label = label.trim().to_string();
        if label.is_empty() {
            return Task::none();
        }
        self.clear_action_error(ErrorScope::Voices);
        Task::perform(rename_voice(id, label), |result| match result {
            Ok(voice) => cosmic::Action::App(Message::Voices(VoicesMessage::Renamed(voice))),
            Err(e) => {
                cosmic::Action::App(Message::Voices(VoicesMessage::RenameFailed(e.to_string())))
            }
        })
    }

    /// Delete one voice, greying its row while the request is in flight.
    fn delete(&mut self, id: String) -> Task<cosmic::Action<Message>> {
        self.clear_action_error(ErrorScope::Voices);
        self.voices.deleting.push(id.clone());
        Task::perform(delete_voice(id.clone()), move |result| {
            cosmic::Action::App(Message::Voices(match result {
                Ok(_) => VoicesMessage::Deleted(id.clone()),
                Err(e) => VoicesMessage::DeleteFailed {
                    id: id.clone(),
                    message: e.to_string(),
                },
            }))
        })
    }

    /// Speak the preview sentence in one voice.
    ///
    /// One at a time. The request stays open until the daemon has registered
    /// the voice with the backend and queued the audio — a first preview can
    /// hold it for the better part of a minute while the backend encodes the
    /// reference clip — and a second request in that window does not queue a
    /// second preview, it cancels the first. So a press that arrives while one
    /// is in flight is dropped rather than served, and the page greys the
    /// buttons to say that before the press happens.
    fn preview(&mut self, voice_id: String) -> Task<cosmic::Action<Message>> {
        if let Some(current) = self.voices.previewing.as_ref() {
            log::debug!("voices: a preview of {current} is already in flight");
            return Task::none();
        }
        // Addressed to the model the card is showing, like every other write
        // here: hearing a voice means selecting it for that model, because a
        // speak request has no voice of its own to carry.
        let Some((_, model)) = self.voice.target.clone() else {
            log::debug!("voices: no model to preview {voice_id} with");
            return Task::none();
        };
        self.clear_action_error(ErrorScope::Voices);
        self.voices.previewing = Some(voice_id.clone());
        Task::perform(
            speak_in_voice(SYNTHESIS_STAGE, model, voice_id, PREVIEW_TEXT.to_string()),
            |result| {
                cosmic::Action::App(Message::Voices(match result {
                    Ok(()) => VoicesMessage::Previewed,
                    Err(e) => VoicesMessage::PreviewFailed(e.to_string()),
                }))
            },
        )
    }

    /// Open the microphone. Any pending sample is dropped first — one sample
    /// at a time keeps "what am I about to save" unambiguous.
    fn start_recording(&mut self) -> Task<cosmic::Action<Message>> {
        if self.voices.is_recording() {
            return Task::none();
        }
        self.clear_action_error(ErrorScope::Voices);
        self.voices.clear_pending();
        self.voices.recording_seconds = 0.0;
        self.voices.recording_level = 0.0;
        self.voices.recorder = Some(Recorder::start(MAX_CLIP_SECONDS));
        Task::none()
    }

    /// Refresh the meter, and stop by ourselves if the recorder already has.
    fn recording_tick(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(recorder) = self.voices.recorder.as_ref() else {
            return Task::none();
        };
        let status = recorder.status();
        self.voices.recording_seconds = status.seconds;
        self.voices.recording_level = status.level;
        if status.finished {
            // Either the cap was reached — in which case the audio is worth
            // keeping — or the device failed, which `stop_recording` reports.
            return self.stop_recording();
        }
        Task::none()
    }

    /// Close the microphone and turn what was captured into a pending sample.
    fn stop_recording(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(recorder) = self.voices.recorder.take() else {
            return Task::none();
        };
        self.voices.recording_level = 0.0;
        let recording = match recorder.finish() {
            Ok(r) => r,
            Err(e) => {
                self.voices.recording_seconds = 0.0;
                self.set_action_error(ErrorScope::Voices, e);
                return Task::none();
            }
        };
        let seconds = recording.seconds();
        match recording.to_wav() {
            Ok(wav) => {
                // A script that was just read is a transcript already written,
                // so fill the field in rather than asking the user to type back
                // words that were on screen a second ago. The field is empty
                // here by construction — `start_recording` cleared it, and
                // there is no transcript field on screen while recording — so
                // there is nothing of the user's to overwrite.
                let script = self
                    .voices
                    .script(self.language.primary_language.as_deref());
                if let Some(script) = script {
                    self.voices.transcript_input = script.text.to_string();
                }
                self.voices.pending = Some(PendingSample {
                    wav,
                    seconds,
                    origin: SampleOrigin::Recorded,
                    script,
                });
            }
            Err(e) => self.set_action_error(ErrorScope::Voices, e),
        }
        self.voices.recording_seconds = 0.0;
        Task::none()
    }

    /// Upload the pending sample.
    fn save_pending(&mut self) -> Task<cosmic::Action<Message>> {
        if self.voices.pending.is_none() {
            return Task::none();
        }
        let label = self.voices.label_input.trim().to_string();
        if label.is_empty() {
            return Task::none();
        }
        let transcript = {
            let t = self.voices.transcript_input.trim();
            (!t.is_empty()).then(|| t.to_string())
        };
        // Backstop for the same rule the Save button already enforces: a
        // model that clones in-context cannot use a clip whose words it was
        // not given, and saying so here beats a `400` after several megabytes
        // have been uploaded.
        if self.voices.needs_transcript() && transcript.is_none() {
            self.set_action_error(
                ErrorScope::Voices,
                "The loaded model clones from the reference transcript, so this voice needs one."
                    .to_string(),
            );
            return Task::none();
        }
        // Cloned rather than taken: a failed upload leaves the sample in place
        // so Save can simply be pressed again, which for a recording that no
        // longer exists anywhere else is the difference between a retry and a
        // re-record.
        let Some(pending) = self.voices.pending.clone() else {
            return Task::none();
        };
        self.voices.saving = true;
        self.clear_action_error(ErrorScope::Voices);
        Task::perform(
            create_voice(label, transcript, pending.wav),
            |result| match result {
                Ok(voice) => cosmic::Action::App(Message::Voices(VoicesMessage::Saved(voice))),
                Err(e) => {
                    cosmic::Action::App(Message::Voices(VoicesMessage::SaveFailed(e.to_string())))
                }
            },
        )
    }
}

/// Read the library and the loaded model's cloning capability.
fn refresh_library() -> Task<cosmic::Action<Message>> {
    Task::perform(list_voices(), |result| match result {
        Ok((voices, model)) => cosmic::Action::App(Message::Voices(VoicesMessage::VoicesLoaded {
            voices,
            model,
        })),
        Err(e) => cosmic::Action::App(Message::Voices(VoicesMessage::LoadFailed(e.to_string()))),
    })
}

/// Pick a WAV file and read it.
///
/// The filter names WAV alone because that is what the daemon stores: offering
/// an mp3 the upload would refuse is worse than not offering it.
async fn pick_wav() -> Result<Option<(String, Vec<u8>)>, String> {
    let Some(handle) = rfd::AsyncFileDialog::new()
        .set_title("Choose a voice recording")
        .add_filter("WAV audio", &["wav"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let name = handle.file_name();
    let bytes = tokio::fs::read(handle.path())
        .await
        .map_err(|e| format!("could not read {name}: {e}"))?;
    Ok(Some((name, bytes)))
}

/// A filename turned into a plausible voice label: the stem, with separators
/// as spaces.
fn label_from_filename(name: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    stem.replace(['_', '-'], " ").trim().to_string()
}

/// Length of a WAV file in seconds, read from its header.
///
/// Zero when the bytes are not a readable WAV — the readout then says nothing
/// rather than guessing, and the daemon is the one that refuses the upload.
fn wav_seconds(wav: &[u8]) -> f32 {
    let Ok(reader) = hound::WavReader::new(std::io::Cursor::new(wav)) else {
        return 0.0;
    };
    let rate = reader.spec().sample_rate;
    if rate == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let (frames, rate) = (reader.duration() as f32, rate as f32);
    frames / rate
}

#[cfg(test)]
mod tests {
    use super::{label_from_filename, wav_seconds};

    #[test]
    fn a_filename_becomes_a_plausible_label() {
        assert_eq!(label_from_filename("ada_lovelace.wav"), "ada lovelace");
        assert_eq!(label_from_filename("my-voice.WAV"), "my voice");
        // No extension, and nothing to replace: the name is the label.
        assert_eq!(label_from_filename("Ada"), "Ada");
    }

    #[test]
    fn a_wav_header_gives_the_length() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for _ in 0..8_000 {
                w.write_sample(0_i16).unwrap();
            }
            w.finalize().unwrap();
        }
        let seconds = wav_seconds(&cursor.into_inner());
        assert!((seconds - 0.5).abs() < 1e-3, "{seconds}");
    }

    /// A file the app cannot read is still uploaded — the daemon is the
    /// authority on what it will accept — so the readout must degrade rather
    /// than panic.
    #[test]
    fn an_unreadable_file_reports_no_length() {
        assert!((wav_seconds(b"not audio") - 0.0).abs() < f32::EPSILON);
    }
}
