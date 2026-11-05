// SPDX-License-Identifier: GPL-3.0-only
//! What `GET /v1/status` reports: each feature's model, device and activity.
//!
//! The daemon serves both features from one process, so a status is one per
//! feature, side by side. A client reads the feature it is about — the
//! dictation hotkey reads `stt.busy`, a speech widget `tts.busy`.

use serde::{Deserialize, Serialize};

/// What `GET /v1/status` answers with.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VoiceStatus {
    /// Always `success`.
    #[cfg_attr(feature = "openapi", schema(example = "success"))]
    pub status: String,
    /// Speech to text. `busy` while a daemon-mic cycle is active — capture
    /// *and* the transcription and typing that follow it. A toggle hotkey reads
    /// it and calls `POST /stt/transcribe/stop` when true, `POST
    /// /stt/transcribe` when false.
    pub stt: FeatureStatus,
    /// Text to speech. `busy` while an utterance is being synthesized or
    /// played.
    pub tts: FeatureStatus,
}

/// One feature's part of a [`VoiceStatus`].
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FeatureStatus {
    /// The accelerator the loaded model actually runs on: `cpu`, `cuda`,
    /// `rocm`, `metal`, `vulkan`, or `remote` for a model served over the
    /// network. `unknown` when nothing is loaded.
    #[cfg_attr(feature = "openapi", schema(example = "cuda"))]
    pub device: String,
    /// `false` while the feature's model is still loading, after a failed
    /// switch, or when none is selected.
    pub model_loaded: bool,
    /// The loaded model's name. Absent when `model_loaded` is `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "whisper-tiny"))]
    pub current_model: Option<String>,
    /// Whether the feature is at work right now. See [`VoiceStatus`].
    pub busy: bool,
}
