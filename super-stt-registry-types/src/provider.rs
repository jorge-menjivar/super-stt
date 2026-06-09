// SPDX-License-Identifier: GPL-3.0-only
//! Model providers: the engine family + routing class a model is served by.
//! The wire form of every variant is `snake_case`.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// An online API provider with guaranteed API configuration.
/// Every variant has a key name and base URL — no panics possible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OnlineProvider {
    OpenAI,
    Mistral,
    Deepgram,
}

impl OnlineProvider {
    /// Provider name used for keyring API key lookups.
    #[must_use]
    pub fn api_key_name(&self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Mistral => "mistral",
            Self::Deepgram => "deepgram",
        }
    }

    /// Base URL for this provider's API.
    #[must_use]
    pub fn api_base_url(&self) -> &'static str {
        match self {
            Self::OpenAI => "https://api.openai.com",
            Self::Mistral => "https://api.mistral.ai",
            Self::Deepgram => "https://api.deepgram.com",
        }
    }
}

impl fmt::Display for OnlineProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenAI => write!(f, "openai"),
            Self::Mistral => write!(f, "mistral"),
            Self::Deepgram => write!(f, "deepgram"),
        }
    }
}

impl FromStr for OnlineProvider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "openai" => Ok(Self::OpenAI),
            "mistral" => Ok(Self::Mistral),
            "deepgram" => Ok(Self::Deepgram),
            _ => Err(format!("Unknown online provider: {s}")),
        }
    }
}

impl Serialize for OnlineProvider {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for OnlineProvider {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// What kind of inference engine a model uses.
///
/// `Provider` carries both the architecture family (Whisper / Voxtral /
/// Online API) and the routing target. Which backend serves the model is a
/// separate axis — see `backend.toml` `[backend].source`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Provider {
    /// Whisper engine, served from local files by a whisper backend.
    #[default]
    LocalWhisper,
    /// Voxtral engine, served from local files by a voxtral backend.
    LocalVoxtral,
    /// Qwen3-ASR engine, served from local files by the qwen3-asr backend.
    LocalQwen3Asr,
    /// Served by an external API.
    Online(OnlineProvider),
}

impl Provider {
    /// Returns the inner `OnlineProvider` if this is an online provider.
    #[must_use]
    pub fn as_online(&self) -> Option<OnlineProvider> {
        match self {
            Self::Online(p) => Some(*p),
            _ => None,
        }
    }
}

impl From<OnlineProvider> for Provider {
    fn from(p: OnlineProvider) -> Self {
        Self::Online(p)
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalWhisper => write!(f, "local_whisper"),
            Self::LocalVoxtral => write!(f, "local_voxtral"),
            Self::LocalQwen3Asr => write!(f, "local_qwen3_asr"),
            Self::Online(p) => write!(f, "{p}"),
        }
    }
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local_whisper" => Ok(Self::LocalWhisper),
            "local_voxtral" => Ok(Self::LocalVoxtral),
            "local_qwen3_asr" => Ok(Self::LocalQwen3Asr),
            other => OnlineProvider::from_str(other).map(Self::Online),
        }
    }
}

impl Serialize for Provider {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
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
            "enum": [
                "local_whisper",
                "local_voxtral",
                "local_qwen3_asr",
                "openai",
                "mistral",
                "deepgram"
            ],
            "description": "Engine that serves the model. The wire form is snake_case."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_providers_match_pattern() {
        assert!(matches!(
            Provider::Online(OnlineProvider::OpenAI),
            Provider::Online(_)
        ));
        assert!(!matches!(Provider::LocalWhisper, Provider::Online(_)));
        assert!(!matches!(Provider::LocalVoxtral, Provider::Online(_)));
        assert!(!matches!(Provider::LocalQwen3Asr, Provider::Online(_)));
    }

    #[test]
    fn as_online_returns_inner() {
        let p = Provider::Online(OnlineProvider::OpenAI);
        assert_eq!(p.as_online(), Some(OnlineProvider::OpenAI));
        assert_eq!(Provider::LocalWhisper.as_online(), None);
    }

    #[test]
    fn from_online_provider() {
        let p: Provider = OnlineProvider::Mistral.into();
        assert_eq!(p, Provider::Online(OnlineProvider::Mistral));
    }

    #[test]
    fn display_round_trips() {
        for provider in [
            Provider::LocalWhisper,
            Provider::LocalVoxtral,
            Provider::LocalQwen3Asr,
            Provider::Online(OnlineProvider::OpenAI),
            Provider::Online(OnlineProvider::Mistral),
            Provider::Online(OnlineProvider::Deepgram),
        ] {
            let s = provider.to_string();
            let parsed: Provider = s.parse().unwrap();
            assert_eq!(provider, parsed);
        }
    }

    #[test]
    fn unknown_provider_strings_are_rejected() {
        // No legacy aliases — bad strings must error so the deserializer can
        // fall back to default at the field level instead of silently
        // accepting a stale format.
        for bad in ["local", "LocalWhisper", "local-whisper", "OpenAI"] {
            assert!(bad.parse::<Provider>().is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn online_provider_display_round_trips() {
        for op in [
            OnlineProvider::OpenAI,
            OnlineProvider::Mistral,
            OnlineProvider::Deepgram,
        ] {
            let s = op.to_string();
            let parsed: OnlineProvider = s.parse().unwrap();
            assert_eq!(op, parsed);
        }
    }

    #[test]
    fn api_key_names() {
        assert_eq!(OnlineProvider::OpenAI.api_key_name(), "openai");
        assert_eq!(OnlineProvider::Mistral.api_key_name(), "mistral");
        assert_eq!(OnlineProvider::Deepgram.api_key_name(), "deepgram");
    }

    #[test]
    fn default_is_local_whisper() {
        assert_eq!(Provider::default(), Provider::LocalWhisper);
    }

    #[test]
    fn provider_serializes_as_canonical_string() {
        assert_eq!(
            serde_json::to_string(&Provider::LocalWhisper).unwrap(),
            "\"local_whisper\""
        );
        assert_eq!(
            serde_json::to_string(&Provider::LocalVoxtral).unwrap(),
            "\"local_voxtral\""
        );
        assert_eq!(
            serde_json::to_string(&Provider::LocalQwen3Asr).unwrap(),
            "\"local_qwen3_asr\""
        );
        assert_eq!(
            serde_json::to_string(&Provider::Online(OnlineProvider::OpenAI)).unwrap(),
            "\"openai\""
        );
        assert_eq!(
            serde_json::to_string(&Provider::Online(OnlineProvider::Mistral)).unwrap(),
            "\"mistral\""
        );
        assert_eq!(
            serde_json::to_string(&Provider::Online(OnlineProvider::Deepgram)).unwrap(),
            "\"deepgram\""
        );
    }

    #[test]
    fn provider_deserializes_canonical_form() {
        for (json, expected) in [
            ("\"local_whisper\"", Provider::LocalWhisper),
            ("\"local_voxtral\"", Provider::LocalVoxtral),
            ("\"local_qwen3_asr\"", Provider::LocalQwen3Asr),
            ("\"openai\"", Provider::Online(OnlineProvider::OpenAI)),
            ("\"mistral\"", Provider::Online(OnlineProvider::Mistral)),
            ("\"deepgram\"", Provider::Online(OnlineProvider::Deepgram)),
        ] {
            let parsed: Provider = serde_json::from_str(json).unwrap();
            assert_eq!(parsed, expected, "json={json}");
        }
    }

    #[test]
    fn provider_rejects_non_canonical_strings() {
        for bad in [
            "\"LocalWhisper\"",
            "\"LocalVoxtral\"",
            "\"local-whisper\"",
            "\"local-voxtral\"",
            "\"local-qwen3-asr\"",
            "\"LocalQwen3Asr\"",
            "\"local\"",
            "\"OpenAI\"",
        ] {
            assert!(
                serde_json::from_str::<Provider>(bad).is_err(),
                "{bad} should fail to deserialize"
            );
        }
    }

    #[test]
    fn online_provider_serializes_as_lowercase() {
        assert_eq!(
            serde_json::to_string(&OnlineProvider::OpenAI).unwrap(),
            "\"openai\""
        );
        assert_eq!(
            serde_json::to_string(&OnlineProvider::Mistral).unwrap(),
            "\"mistral\""
        );
        assert_eq!(
            serde_json::to_string(&OnlineProvider::Deepgram).unwrap(),
            "\"deepgram\""
        );
    }
}

#[cfg(test)]
#[cfg(feature = "schema")]
mod schema_tests {
    use super::{OnlineProvider, Provider};

    /// The hand-written schema enum must stay in lock-step with `FromStr` —
    /// every schema value parses, and every variant's Display is in the schema.
    #[test]
    fn schema_enum_matches_fromstr() {
        let schema = serde_json::to_value(schemars::schema_for!(Provider)).unwrap();
        let values: Vec<String> = schema["enum"]
            .as_array()
            .expect("enum array")
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        for v in &values {
            v.parse::<Provider>()
                .unwrap_or_else(|e| panic!("schema value {v:?} must parse: {e}"));
        }
        for p in [
            Provider::LocalWhisper,
            Provider::LocalVoxtral,
            Provider::LocalQwen3Asr,
            Provider::Online(OnlineProvider::OpenAI),
            Provider::Online(OnlineProvider::Mistral),
            Provider::Online(OnlineProvider::Deepgram),
        ] {
            assert!(
                values.contains(&p.to_string()),
                "{p} missing from schema enum"
            );
        }
    }
}
