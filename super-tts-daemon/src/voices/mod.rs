// SPDX-License-Identifier: GPL-3.0-only
//! The voice library: reference clips a user cloned a voice from.
//!
//! A cloned voice is two things — a recording of somebody speaking, and the
//! id (`voice:<uuid>`) that names it on `POST /speak`. This module owns both,
//! and owns them independently of any model: a voice outlives the backend that
//! was loaded when it was added, and the same clip is registered against
//! whatever model is loaded next.
//!
//! **One canonical shape.** Every clip is stored mono, 16-bit, at
//! [`CLIP_SAMPLE_RATE`], whatever it arrived as. Backends then receive one
//! predictable format instead of each carrying a resampler for the rates a
//! phone, a headset and a studio interface happen to produce — and 24 kHz is
//! what the speaker encoders in scope actually want.
//!
//! **The clip is stored whole and trimmed per model.** `clone_ref_seconds` is
//! a property of the model, not of the recording: one model takes 30 seconds
//! and the next takes 10. Storing the full clip and trimming at registration
//! means switching models never asks the user to record again.
//!
//! **Layout.** One directory per voice under `$XDG_DATA_HOME/super-tts/voices`:
//! `<uuid>/clip.wav` beside `<uuid>/voice.json`. A directory rather than a
//! shared index file so adding and deleting a voice is a create and an rmdir,
//! with no index to rewrite and no two writers to serialize.

pub mod wav;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Rate every stored clip is resampled to. Matches the mel front-end of the
/// speaker encoders these clips are destined for, so the common case costs no
/// second resampling inside the backend.
pub const CLIP_SAMPLE_RATE: u32 = 24_000;

/// Channel count of a stored clip. Reference audio is one person speaking;
/// there is no stereo image to preserve.
pub const CLIP_CHANNELS: u16 = 1;

/// Wire name of the sample format a registered clip is delivered in, matching
/// the `x-tts-format` vocabulary of the synthesis response.
pub const CLIP_FORMAT: &str = "s16le";

/// Longest clip the library will store, in seconds.
///
/// Generous next to any model's `clone_ref_seconds` (tens of seconds at most)
/// because the library is the archive: a user may keep a two-minute recording
/// and let each model take the prefix it wants.
pub const MAX_CLIP_SECONDS: f32 = 120.0;

/// Largest upload accepted for one clip, before decoding.
///
/// [`MAX_CLIP_SECONDS`] of 48 kHz stereo 24-bit PCM is about 41 MB; the cap
/// sits above that so the duration check — which reports what is actually
/// wrong — is the one users hit, rather than a byte count that reads like a
/// bug.
pub const MAX_UPLOAD_BYTES: usize = 48 * 1024 * 1024;

/// Longest voice label accepted.
pub const MAX_LABEL_CHARS: usize = 128;

/// Longest reference transcript accepted. A transcript describes a clip of at
/// most [`MAX_CLIP_SECONDS`]; anything past this is not a transcript.
pub const MAX_TRANSCRIPT_CHARS: usize = 4_000;

/// Prefix marking a cloned voice id, per `docs/protocol/backend/config.md`.
pub const VOICE_ID_PREFIX: &str = "voice:";

/// Strip the `voice:` prefix from an id, if present.
#[must_use]
pub fn strip_prefix(voice: &str) -> Option<&str> {
    voice.strip_prefix(VOICE_ID_PREFIX)
}

/// One cloned voice: what the library knows about it apart from the audio.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Voice {
    /// The uuid half of the voice id. `voice:<id>` is what `/speak` takes.
    pub id: String,
    /// Display name, chosen by the user.
    pub label: String,
    /// What the clip says, when it was supplied.
    ///
    /// Optional because only in-context cloning needs it — speaker-embedding
    /// cloning conditions on the audio alone. Models that need it declare
    /// `clone_needs_transcript`, and the daemon checks before registering
    /// rather than letting synthesis fail later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// RFC 3339 creation time.
    pub created_at: String,
    /// Length of the stored clip in seconds, after downmix and resampling.
    pub duration_seconds: f32,
    /// Always [`CLIP_SAMPLE_RATE`]; recorded so a future change of the
    /// canonical rate can be detected on clips written before it.
    pub sample_rate: u32,
    /// Always [`CLIP_CHANNELS`], recorded for the same reason.
    pub channels: u16,
}

impl Voice {
    /// The wire id a client passes as `voice` on `POST /speak`.
    #[must_use]
    pub fn voice_id(&self) -> String {
        format!("{VOICE_ID_PREFIX}{}", self.id)
    }
}

/// A stored clip, decoded and trimmed for one model.
#[derive(Debug, Clone)]
pub struct Clip {
    /// Little-endian 16-bit PCM, mono, at [`CLIP_SAMPLE_RATE`].
    pub pcm: Vec<u8>,
    /// Length of `pcm` in seconds.
    pub seconds: f32,
}

/// Why a voice-library operation could not be completed.
#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    /// A field was missing, empty, or past its bound.
    #[error("{0}")]
    InvalidValue(String),
    /// The uploaded audio could not be used.
    #[error("{0}")]
    Audio(#[from] wav::WavError),
    /// No voice with that id.
    #[error("no such voice")]
    NotFound,
    /// The library could not be read or written.
    #[error("voice library unavailable: {0}")]
    Io(String),
}

/// The on-disk voice library, rooted at one directory.
#[derive(Debug, Clone)]
pub struct VoiceLibrary {
    dir: PathBuf,
}

impl VoiceLibrary {
    /// A library rooted at `dir`. The directory is created on first write, not
    /// here: a daemon that never clones a voice leaves no trace.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The default location, `$XDG_DATA_HOME/super-tts/voices`.
    #[must_use]
    pub fn default_dir() -> PathBuf {
        super_tts_shared::paths::data_dir().join("voices")
    }

    /// The directory this library reads and writes.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Add a voice from an uploaded WAV file.
    ///
    /// Blocking and CPU-bound — it decodes and resamples the whole clip — so
    /// callers on the async runtime must go through `spawn_blocking`.
    ///
    /// # Errors
    /// [`VoiceError::InvalidValue`] for a bad label or transcript,
    /// [`VoiceError::Audio`] for audio that cannot be read or is too long, and
    /// [`VoiceError::Io`] if the clip cannot be written.
    pub fn create(
        &self,
        audio: &[u8],
        label: &str,
        transcript: Option<&str>,
    ) -> Result<Voice, VoiceError> {
        let label = label.trim();
        if label.is_empty() {
            return Err(VoiceError::InvalidValue("label must not be empty".into()));
        }
        if label.chars().count() > MAX_LABEL_CHARS {
            return Err(VoiceError::InvalidValue(format!(
                "label is longer than {MAX_LABEL_CHARS} characters"
            )));
        }
        let transcript = match transcript.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) if t.chars().count() > MAX_TRANSCRIPT_CHARS => {
                return Err(VoiceError::InvalidValue(format!(
                    "transcript is longer than {MAX_TRANSCRIPT_CHARS} characters"
                )));
            }
            other => other.map(str::to_owned),
        };
        if audio.len() > MAX_UPLOAD_BYTES {
            return Err(VoiceError::InvalidValue(format!(
                "audio is larger than {MAX_UPLOAD_BYTES} bytes"
            )));
        }

        let decoded = wav::decode(audio, MAX_CLIP_SECONDS)?;
        let samples = if decoded.sample_rate == CLIP_SAMPLE_RATE {
            decoded.samples
        } else {
            super_tts_shared::utils::audio::resample(
                &decoded.samples,
                decoded.sample_rate,
                CLIP_SAMPLE_RATE,
                super_tts_shared::utils::audio::ResampleQuality::HighQuality,
            )
            .map_err(|e| VoiceError::Io(format!("resampling the clip failed: {e}")))?
        };
        if samples.is_empty() {
            return Err(VoiceError::InvalidValue("clip contains no audio".into()));
        }
        let bytes = wav::encode_s16(&samples, CLIP_SAMPLE_RATE)?;

        let voice = Voice {
            id: uuid::Uuid::new_v4().to_string(),
            label: label.to_string(),
            transcript,
            created_at: chrono::Utc::now().to_rfc3339(),
            duration_seconds: crate::num_cast::usize_to_f32(samples.len())
                / crate::num_cast::u32_to_f32(CLIP_SAMPLE_RATE),
            sample_rate: CLIP_SAMPLE_RATE,
            channels: CLIP_CHANNELS,
        };

        let dir = self.dir.join(&voice.id);
        std::fs::create_dir_all(&dir).map_err(|e| VoiceError::Io(e.to_string()))?;
        restrict(&self.dir, 0o700);
        restrict(&dir, 0o700);
        let clip = dir.join("clip.wav");
        write_private(&clip, &bytes)?;
        let meta = serde_json::to_vec_pretty(&voice)
            .map_err(|e| VoiceError::Io(format!("serializing voice metadata: {e}")))?;
        if let Err(e) = write_private(&dir.join("voice.json"), &meta) {
            // A clip with no metadata beside it would be listed as nothing and
            // occupy its id forever, so the half-written voice goes away.
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        Ok(voice)
    }

    /// Every stored voice, newest first.
    ///
    /// A directory that cannot be read as a voice is skipped rather than
    /// failing the listing: one corrupt entry must not hide the rest.
    ///
    /// # Errors
    /// [`VoiceError::Io`] if the library directory itself cannot be read. A
    /// library that has never been written to lists empty.
    pub fn list(&self) -> Result<Vec<Voice>, VoiceError> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(VoiceError::Io(e.to_string())),
        };
        let mut voices: Vec<Voice> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| read_voice(&entry.path().join("voice.json")).ok())
            .collect();
        voices.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(voices)
    }

    /// One voice's metadata.
    ///
    /// # Errors
    /// [`VoiceError::NotFound`] if `id` names no voice, or is not a uuid at
    /// all — the same answer either way, so a malformed id cannot be used to
    /// probe the filesystem.
    pub fn get(&self, id: &str) -> Result<Voice, VoiceError> {
        let dir = self.voice_dir(id)?;
        read_voice(&dir.join("voice.json"))
    }

    /// Rename a voice.
    ///
    /// The transcript is deliberately not editable: it describes the stored
    /// audio, backends cache what they derived from the pair, and a changed
    /// transcript would silently disagree with every registration already
    /// made. Re-record instead.
    ///
    /// # Errors
    /// [`VoiceError::NotFound`], [`VoiceError::InvalidValue`] for an empty or
    /// over-long label, or [`VoiceError::Io`] if the metadata cannot be
    /// rewritten.
    pub fn rename(&self, id: &str, label: &str) -> Result<Voice, VoiceError> {
        let label = label.trim();
        if label.is_empty() {
            return Err(VoiceError::InvalidValue("label must not be empty".into()));
        }
        if label.chars().count() > MAX_LABEL_CHARS {
            return Err(VoiceError::InvalidValue(format!(
                "label is longer than {MAX_LABEL_CHARS} characters"
            )));
        }
        let dir = self.voice_dir(id)?;
        let mut voice = read_voice(&dir.join("voice.json"))?;
        voice.label = label.to_string();
        let meta = serde_json::to_vec_pretty(&voice)
            .map_err(|e| VoiceError::Io(format!("serializing voice metadata: {e}")))?;
        write_private(&dir.join("voice.json"), &meta)?;
        Ok(voice)
    }

    /// Delete a voice and its clip.
    ///
    /// # Errors
    /// [`VoiceError::NotFound`] if `id` names no voice, [`VoiceError::Io`] if
    /// the directory cannot be removed.
    pub fn delete(&self, id: &str) -> Result<(), VoiceError> {
        let dir = self.voice_dir(id)?;
        if !dir.join("voice.json").is_file() {
            return Err(VoiceError::NotFound);
        }
        std::fs::remove_dir_all(&dir).map_err(|e| VoiceError::Io(e.to_string()))
    }

    /// The stored clip as a WAV file, for a client that wants to play it back.
    ///
    /// # Errors
    /// [`VoiceError::NotFound`] or [`VoiceError::Io`].
    pub fn clip_wav(&self, id: &str) -> Result<Vec<u8>, VoiceError> {
        let dir = self.voice_dir(id)?;
        match std::fs::read(dir.join("clip.wav")) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(VoiceError::NotFound),
            Err(e) => Err(VoiceError::Io(e.to_string())),
        }
    }

    /// The stored clip as raw PCM, trimmed to `max_seconds` when the model
    /// asks for less than the recording holds.
    ///
    /// Trimming takes the beginning of the clip. A prefix is what a user can
    /// predict and re-record against; picking a window from the middle would
    /// make "why does it sound like that" unanswerable.
    ///
    /// Blocking: reads and decodes the stored file.
    ///
    /// # Errors
    /// [`VoiceError::NotFound`], [`VoiceError::Io`], or [`VoiceError::Audio`]
    /// if the stored clip no longer decodes.
    pub fn clip_pcm(&self, id: &str, max_seconds: Option<f32>) -> Result<Clip, VoiceError> {
        let bytes = self.clip_wav(id)?;
        let mut decoded = wav::decode(&bytes, MAX_CLIP_SECONDS)?;
        if let Some(max) = max_seconds.filter(|m| m.is_finite() && *m > 0.0) {
            let keep =
                crate::num_cast::f32_to_usize(max * crate::num_cast::u32_to_f32(CLIP_SAMPLE_RATE));
            if decoded.samples.len() > keep {
                decoded.samples.truncate(keep);
            }
        }
        Ok(Clip {
            seconds: decoded.seconds(),
            pcm: wav::to_s16le(&decoded.samples),
        })
    }

    /// The directory holding `id`, rejecting anything that is not a uuid.
    ///
    /// The uuid check is the path-traversal guard: ids arrive from a client,
    /// and `..` must never reach a `join`.
    fn voice_dir(&self, id: &str) -> Result<PathBuf, VoiceError> {
        let id = strip_prefix(id).unwrap_or(id);
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(VoiceError::NotFound);
        }
        Ok(self.dir.join(id))
    }
}

/// Read and parse one `voice.json`.
fn read_voice(path: &Path) -> Result<Voice, VoiceError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(VoiceError::NotFound),
        Err(e) => return Err(VoiceError::Io(e.to_string())),
    };
    serde_json::from_slice(&bytes).map_err(|e| VoiceError::Io(format!("unreadable metadata: {e}")))
}

/// Write `bytes` to `path`, readable only by the owner.
///
/// A reference clip is a recording of somebody's voice. It gets the same
/// treatment as the session store, not the default umask.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), VoiceError> {
    std::fs::write(path, bytes).map_err(|e| VoiceError::Io(e.to_string()))?;
    restrict(path, 0o600);
    Ok(())
}

/// Best-effort `chmod`. A library that is readable is still usable, so a
/// failure here is not worth failing the write over — but it is worth logging.
fn restrict(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
        log::warn!("could not restrict {} to {mode:o}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests;
