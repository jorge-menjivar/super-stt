// SPDX-License-Identifier: GPL-3.0-only
//! The headers the daemon hands a backend on its requests: which model a
//! request is for, the user's secrets and options for the backend, and the
//! user's dictation context.
//!
//! Each is named `x-voice-<name>`. Backends built before those names read the
//! same value under their feature's prefix, `x-stt-<name>` or `x-tts-<name>`,
//! so the daemon sends every header under both names.

/// The prefix of every header the daemon hands a backend.
pub const PREFIX: &str = "x-voice-";

/// Which feature a backend request is for. It decides the prefix the
/// header's former name has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// Speech to text, whose headers were `x-stt-*`.
    SpeechToText,
    /// Text to speech, whose headers were `x-tts-*`.
    TextToSpeech,
}

impl Feature {
    /// The prefix this feature's headers had before they were `x-voice-*`.
    #[must_use]
    pub const fn former_prefix(self) -> &'static str {
        match self {
            Self::SpeechToText => "x-stt-",
            Self::TextToSpeech => "x-tts-",
        }
    }
}

/// Add the header `name`, given without a prefix (`model`,
/// `secret-api_key`), with `value`, under both of its names.
pub fn push(headers: &mut Vec<(String, String)>, feature: Feature, name: &str, value: String) {
    headers.push((format!("{PREFIX}{name}"), value.clone()));
    headers.push((format!("{}{name}", feature.former_prefix()), value));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_goes_under_its_name_and_its_former_one() {
        let mut headers = Vec::new();
        push(
            &mut headers,
            Feature::SpeechToText,
            "model",
            "whisper-base".into(),
        );
        push(
            &mut headers,
            Feature::TextToSpeech,
            "secret-api_key",
            "k".into(),
        );
        assert_eq!(
            headers,
            [
                ("x-voice-model".to_string(), "whisper-base".to_string()),
                ("x-stt-model".to_string(), "whisper-base".to_string()),
                ("x-voice-secret-api_key".to_string(), "k".to_string()),
                ("x-tts-secret-api_key".to_string(), "k".to_string()),
            ]
        );
    }
}
