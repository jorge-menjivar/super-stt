// SPDX-License-Identifier: GPL-3.0-only

//! The `/v1/voice` cloned-voice library responses.
//!
//! A cloned voice is a reference recording plus the id that names it on
//! `POST /speak`. The daemon serializes these shapes from its on-disk library;
//! the settings UI deserializes them and renders one card per voice. Keeping
//! the shape here, shared by both sides, is what keeps the wire contract from
//! drifting — the same arrangement [`backends`](super::backends) uses.
//!
//! The daemon's stored `voice.json` is deliberately **not** this type. Storage
//! and wire agree today, and separating them costs one small conversion, but
//! the alternative is a file format that cannot change without changing the
//! protocol.
//!
//! `#[serde(default)]` on the non-identity fields lets an older daemon that
//! omits a newer field still deserialize.

use serde::{Deserialize, Serialize};

/// One stored voice, as a client sees it.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VoiceInfo {
    /// The uuid half of the id, for building paths under `/voice`.
    pub id: String,
    /// The full wire id, `voice:<uuid>` — what `POST /speak` takes as `voice`.
    /// Sent rather than left to the client to assemble, so the prefix is
    /// defined in one place.
    pub voice_id: String,
    /// Display name the user chose.
    pub label: String,
    /// What the clip says, when it was supplied. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// RFC 3339 creation time.
    #[serde(default)]
    pub created_at: String,
    /// Length of the stored recording, after downmix and resampling. The whole
    /// recording — a model may use only the prefix its `clone_ref_seconds`
    /// allows.
    #[serde(default)]
    pub duration_seconds: f32,
    /// Sample rate of the stored clip.
    #[serde(default)]
    pub sample_rate: u32,
    /// Channel count of the stored clip.
    #[serde(default)]
    pub channels: u16,
}

/// What the currently loaded model can do with the library.
///
/// Carried on the listing because it is what a voices UI needs and nothing
/// else does: whether to offer cloning at all, how much of a recording will
/// actually be used, and whether to ask the user what the clip says.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VoiceModelSupport {
    /// Wire name of the loaded model.
    pub name: String,
    /// Repo id of the backend serving it.
    #[serde(default)]
    pub source: String,
    /// Whether it accepts `voice:<uuid>` ids at all.
    pub clones: bool,
    /// Longest reference audio it takes, in seconds. `None` when it does not
    /// clone.
    #[serde(default)]
    pub clone_ref_seconds: Option<f32>,
    /// Whether registering a voice with it also requires the clip's
    /// transcript.
    #[serde(default)]
    pub needs_transcript: bool,
}

/// `GET /voice/list`.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VoiceListResponse {
    /// `"success"`.
    pub status: String,
    /// Every stored voice, newest first.
    #[serde(default)]
    pub voices: Vec<VoiceInfo>,
    /// The loaded model's cloning capability; `None` when nothing is loaded.
    #[serde(default)]
    pub model: Option<VoiceModelSupport>,
}

/// `POST /voice`, `GET /voice/{id}`, and `PATCH /voice/{id}`.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VoiceResponse {
    /// `"success"`.
    pub status: String,
    /// The created, read, or renamed voice.
    pub voice: VoiceInfo,
}

/// `DELETE /voice/{id}`.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VoiceDeletedResponse {
    /// `"success"`.
    pub status: String,
    /// The wire id that was removed.
    #[serde(default)]
    pub deleted: String,
}
