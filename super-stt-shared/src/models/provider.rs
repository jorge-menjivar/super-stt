// SPDX-License-Identifier: GPL-3.0-only
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// An online API provider with guaranteed API configuration.
/// Every variant has a key name and base URL — no panics possible.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// What kind of inference engine a model uses.
///
/// `Provider` carries both the architecture family (Whisper / Voxtral /
/// Online API) and the routing target. The storage location (HF cache,
/// local disk, no files) is separate — see [`crate::models::registry::ModelSource`].
/// Custom user-provided models use whichever local variant matches the
/// architecture detected from their `config.json`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Provider {
    /// Whisper engine, served from local files (HF cache or `custom_models_dir`).
    #[default]
    LocalWhisper,
    /// Voxtral engine, served from local files (HF cache or `custom_models_dir`).
    LocalVoxtral,
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
            Self::LocalWhisper => write!(f, "local-whisper"),
            Self::LocalVoxtral => write!(f, "local-voxtral"),
            Self::Online(p) => write!(f, "{p}"),
        }
    }
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local-whisper" | "local" => Ok(Self::LocalWhisper),
            "local-voxtral" => Ok(Self::LocalVoxtral),
            other => OnlineProvider::from_str(other).map(Self::Online),
        }
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
    fn legacy_local_alias_parses() {
        // "local" used to be the wire string for the default local variant.
        // Keep accepting it for compat with existing callers/configs.
        assert_eq!("local".parse::<Provider>().unwrap(), Provider::LocalWhisper);
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
}
