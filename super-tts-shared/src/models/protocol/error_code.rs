// SPDX-License-Identifier: GPL-3.0-only
//! Stable, machine-readable error identifiers carried by `DaemonResponse`
//! error responses (the `error_code` field). See `docs/protocol/transport.md`.
//!
//! `error_code` — not the free-form `message` — is the field clients switch on,
//! and the single source of truth for the code→HTTP-status mapping. Serializes
//! to `snake_case` on the wire; an unrecognized code (e.g. from a newer daemon)
//! deserializes to [`ErrorCode::Unknown`] so older clients degrade gracefully.

use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    // --- 409 Conflict: the request is well-formed but the daemon's current
    // state forbids it. ---
    /// An utterance was being synthesized or played, so a mutation that needs
    /// the model (backend switch, model switch, reload, unload, device switch)
    /// was refused.
    ///
    /// No longer emitted: those mutations now stop the utterance and go ahead,
    /// the way a new `POST /speak` always preempted the current one. Kept so a
    /// client still talking to an older daemon parses it as itself rather than
    /// as [`ErrorCode::Unknown`].
    SpeechInProgress,
    /// A model download/switch is already in flight.
    DownloadInProgress,
    /// A cancel was requested but there is no switch/download to cancel
    /// (`POST /active_model/cancel`; see `active_model/cancel.md`).
    NoSwitchInProgress,
    /// No model is loaded, so nothing can be synthesized. The request is
    /// well-formed and succeeds once a model is loaded via `POST /active_model`.
    ModelNotLoaded,

    // --- 400 Bad Request: the client gave the daemon something it couldn't
    // use. ---
    /// No installed backend serves the named model.
    InvalidModel,
    /// No installed backend has the named source.
    InvalidBackend,
    /// A CUDA device was requested but CUDA is unavailable. Reserved: the daemon
    /// currently falls back to CPU silently rather than emitting this (see
    /// `active_device.md`).
    CudaUnavailable,
    /// The requested `device` wasn't one the daemon accepts (`cpu`/`cuda`).
    InvalidDevice,
    /// An online model was requested while online models are disabled.
    OnlineModelsDisabled,
    /// An unrecognized audio-theme name.
    InvalidAudioTheme,
    /// A language tag the model does not support (or the model isn't
    /// multilingual).
    UnsupportedLanguage,
    /// A field failed validation (missing, wrong type, or out of range).
    InvalidValue,

    // --- Other classes. ---
    /// The addressed resource does not exist.
    NotFound,
    /// An unclassified server-side failure (the default for un-coded errors).
    Internal,

    /// An `error_code` this build does not recognize (forward compatibility).
    #[serde(other)]
    Unknown,
}

impl ErrorCode {
    /// The HTTP status this code maps to — the single source of truth for the
    /// code→status contract documented in `docs/protocol/transport.md`.
    #[must_use]
    pub fn http_status(self) -> u16 {
        match self {
            Self::SpeechInProgress
            | Self::DownloadInProgress
            | Self::NoSwitchInProgress
            | Self::ModelNotLoaded => 409,
            Self::InvalidModel
            | Self::InvalidBackend
            | Self::CudaUnavailable
            | Self::InvalidDevice
            | Self::OnlineModelsDisabled
            | Self::InvalidAudioTheme
            | Self::UnsupportedLanguage
            | Self::InvalidValue => 400,
            Self::NotFound => 404,
            Self::Internal | Self::Unknown => 500,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ErrorCode;

    #[test]
    fn serializes_to_snake_case() {
        let json = serde_json::to_string(&ErrorCode::SpeechInProgress).unwrap();
        assert_eq!(json, r#""speech_in_progress""#);
    }

    #[test]
    fn unknown_code_deserializes_to_unknown() {
        let code: ErrorCode = serde_json::from_str(r#""some_future_code""#).unwrap();
        assert_eq!(code, ErrorCode::Unknown);
        assert_eq!(code.http_status(), 500);
    }

    #[test]
    fn status_mapping_is_stable() {
        assert_eq!(ErrorCode::SpeechInProgress.http_status(), 409);
        assert_eq!(ErrorCode::InvalidAudioTheme.http_status(), 400);
        assert_eq!(ErrorCode::NotFound.http_status(), 404);
        assert_eq!(ErrorCode::Internal.http_status(), 500);
    }

    /// `model_not_loaded` is a state conflict, not bad input: the request is
    /// well-formed and becomes valid once a model is loaded. It shares the 409
    /// group with the other "daemon's current state forbids it" codes.
    #[test]
    fn model_not_loaded_is_a_state_conflict() {
        assert_eq!(ErrorCode::ModelNotLoaded.http_status(), 409);
        assert_eq!(
            serde_json::to_string(&ErrorCode::ModelNotLoaded).unwrap(),
            r#""model_not_loaded""#
        );
    }
}
