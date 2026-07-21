// SPDX-License-Identifier: GPL-3.0-only
//! Fixed notices typed into the focused window when a write-mode recording
//! fails.
//!
//! These are compile-time constants rather than formatted error text on
//! purpose. Backends are explicitly untrusted (audit 2 Tier 3 #8) and most
//! failure detail originates in one, so typing it would put
//! attacker-influencable text into whatever the user has focused. The detail
//! goes to the log and the error response; only these fixed strings are typed.

/// No model is loaded, so the cycle cannot produce text. Caught before capture.
pub(crate) const NO_MODEL_LOADED: &str = "[Super STT: no model loaded]";

/// The recorder could not be spawned; capture never began.
pub(crate) const COULD_NOT_START_RECORDING: &str = "[Super STT: could not start recording]";

/// Capture began but failed partway through.
pub(crate) const RECORDING_FAILED: &str = "[Super STT: recording failed]";

/// Audio was captured but the model failed to transcribe it.
pub(crate) const TRANSCRIPTION_FAILED: &str = "[Super STT: transcription failed]";

#[cfg(test)]
pub(crate) const ALL: &[&str] = &[
    NO_MODEL_LOADED,
    COULD_NOT_START_RECORDING,
    RECORDING_FAILED,
    TRANSCRIPTION_FAILED,
];
