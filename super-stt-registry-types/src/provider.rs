// SPDX-License-Identifier: GPL-3.0-only
//! The engine that serves a model. A free-form `snake_case` identifier — any
//! backend may define its own provider (`local_whisper`, `openai`, `groq`, …);
//! the type only constrains the shape, not the set of values.
//!
//! Whether a model is online (served by a remote API with no local compute) is
//! **not** encoded here — it is the `none` sentinel in the model's
//! `supported_devices` (see `ModelEntry::is_online` and
//! `docs/protocol/backend/config.md`).
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A model provider: a `snake_case` engine identifier. Free-form so a backend
/// can serve any provider; only the shape is constrained
/// (`[a-z][a-z0-9_]*`). The empty string is the "unset" sentinel used by
/// defaults (e.g. a daemon's preferred provider before one is chosen).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Provider(String);

impl Provider {
    /// The provider string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the provider is unset (the empty sentinel).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether `s` is a well-formed provider identifier: non-empty, starting
    /// with an ASCII lowercase letter, then lowercase letters, digits, or `_`.
    #[must_use]
    pub fn is_valid(s: &str) -> bool {
        let mut chars = s.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Provider {
    type Err = String;

    /// Parse external input. The empty string (unset) and any well-formed
    /// `snake_case` identifier are accepted; a malformed non-empty value is
    /// rejected so a typo cannot masquerade as a provider.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() || Self::is_valid(s) {
            Ok(Self(s.to_string()))
        } else {
            Err(format!(
                "invalid provider {s:?}: expected a snake_case identifier [a-z][a-z0-9_]*"
            ))
        }
    }
}

impl From<&str> for Provider {
    /// Infallible construction for trusted/internal values (defaults, tests).
    /// External input goes through [`FromStr`]/[`Deserialize`], which validate.
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl Serialize for Provider {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Provider {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for Provider {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Provider".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^[a-z][a-z0-9_]*$",
            "description": "Engine that serves the model — a free-form snake_case identifier (e.g. `local_whisper`, `openai`, `groq`). Any backend may define its own; online vs local is determined by the model's `supported_devices`, not by this value."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_any_snake_case_identifier() {
        for s in ["local_whisper", "openai", "groq", "my_custom_engine", "x", "a1_b2"] {
            assert_eq!(s.parse::<Provider>().unwrap().as_str(), s);
        }
    }

    #[test]
    fn rejects_malformed_non_empty_values() {
        // Uppercase, hyphens, a leading digit/underscore, spaces, non-ASCII.
        for bad in ["OpenAI", "local-whisper", "1foo", "has space", "_lead", "Café"] {
            assert!(bad.parse::<Provider>().is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn empty_is_the_unset_sentinel() {
        assert!(Provider::default().is_empty());
        assert!("".parse::<Provider>().unwrap().is_empty());
        assert_eq!(Provider::default().to_string(), "");
    }

    #[test]
    fn round_trips_through_serde_as_a_plain_string() {
        let p: Provider = "openai".parse().unwrap();
        assert_eq!(serde_json::to_string(&p).unwrap(), "\"openai\"");
        let back: Provider = serde_json::from_str("\"openai\"").unwrap();
        assert_eq!(back, p);
        // Empty round-trips (the unset sentinel must survive a config save/load).
        let empty: Provider = serde_json::from_str("\"\"").unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn deserialize_rejects_malformed() {
        assert!(serde_json::from_str::<Provider>("\"local-whisper\"").is_err());
        assert!(serde_json::from_str::<Provider>("\"OpenAI\"").is_err());
    }
}
