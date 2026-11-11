// SPDX-License-Identifier: GPL-3.0-only
//! Keeping secret values out of anything the daemon prints.
//!
//! A backend's secrets reach it as request headers named
//! `x-<prefix>-secret-<name>`. A type holding such headers prints them
//! through [`Redacted`], which keeps each header's name and hides a secret's
//! value, so a log line or an error that formats one with `{:?}` shows which
//! secrets were set, never what they are.

use std::fmt;

/// What a secret's value prints as.
pub const HIDDEN: &str = "<redacted>";

/// Whether the header `name` carries a secret.
#[must_use]
pub fn is_secret(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("x-") && name.contains("-secret-")
}

/// Request headers as `Debug` prints them: each name with its value, except a
/// secret's, which is [`HIDDEN`].
pub struct Redacted<'a>(pub &'a [(String, String)]);

impl fmt::Debug for Redacted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(name, value)| {
                let value = if is_secret(name) {
                    HIDDEN
                } else {
                    value.as_str()
                };
                (name.as_str(), value)
            }))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_header_is_one_named_secret_under_any_prefix() {
        for name in [
            "x-stt-secret-OPENAI_API_KEY",
            "x-tts-secret-api_key",
            "x-voice-secret-api_key",
            "X-STT-SECRET-token",
        ] {
            assert!(is_secret(name), "{name}");
        }
        for name in [
            "x-stt-option-base_url",
            "x-stt-model",
            "content-type",
            "secret",
        ] {
            assert!(!is_secret(name), "{name}");
        }
    }

    #[test]
    fn a_secret_prints_hidden_and_everything_else_as_it_is() {
        let headers = [
            ("x-stt-model".to_string(), "whisper-1".to_string()),
            (
                "x-stt-secret-OPENAI_API_KEY".to_string(),
                "sk-live-123".to_string(),
            ),
            (
                "x-stt-option-base_url".to_string(),
                "https://api.openai.com".to_string(),
            ),
        ];
        let printed = format!("{:?}", Redacted(&headers));
        assert!(!printed.contains("sk-live-123"), "{printed}");
        assert!(printed.contains("x-stt-secret-OPENAI_API_KEY"), "{printed}");
        assert!(printed.contains(HIDDEN), "{printed}");
        assert!(printed.contains("whisper-1"), "{printed}");
        assert!(printed.contains("https://api.openai.com"), "{printed}");
    }

    #[test]
    fn a_backend_context_prints_without_its_secrets() {
        let headers = vec![
            (
                "x-stt-secret-OPENAI_API_KEY".to_string(),
                "sk-live-123".to_string(),
            ),
            (
                "x-tts-secret-api_key".to_string(),
                "tts-key-456".to_string(),
            ),
        ];
        let printed = [
            format!(
                "{:?}",
                crate::stt_models::transcribe::BackendContext {
                    headers: headers.clone(),
                    user_allowed_hosts: vec!["gateway.example".to_string()],
                }
            ),
            format!(
                "{:?}",
                crate::tts::tts_models::synthesize::BackendContext {
                    headers,
                    user_allowed_hosts: vec!["gateway.example".to_string()],
                }
            ),
        ];
        for printed in printed {
            assert!(!printed.contains("sk-live-123"), "{printed}");
            assert!(!printed.contains("tts-key-456"), "{printed}");
            assert!(printed.contains("gateway.example"), "{printed}");
        }
    }
}
