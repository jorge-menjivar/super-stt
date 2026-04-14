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

/// How a model is served: locally or via an online API provider.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Provider {
    #[default]
    Local,
    Online(OnlineProvider),
}

impl Provider {
    #[must_use]
    pub fn is_online(&self) -> bool {
        matches!(self, Self::Online(_))
    }

    /// Returns the inner `OnlineProvider` if this is an online provider.
    #[must_use]
    pub fn as_online(&self) -> Option<OnlineProvider> {
        match self {
            Self::Online(p) => Some(*p),
            Self::Local => None,
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
            Self::Local => write!(f, "local"),
            Self::Online(p) => write!(f, "{p}"),
        }
    }
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local" => Ok(Self::Local),
            other => OnlineProvider::from_str(other).map(Self::Online),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_is_not_online() {
        assert!(!Provider::Local.is_online());
        assert!(Provider::Local.as_online().is_none());
    }

    #[test]
    fn online_providers_are_online() {
        assert!(Provider::Online(OnlineProvider::OpenAI).is_online());
        assert!(Provider::Online(OnlineProvider::Mistral).is_online());
        assert!(Provider::Online(OnlineProvider::Deepgram).is_online());
    }

    #[test]
    fn as_online_returns_inner() {
        let p = Provider::Online(OnlineProvider::OpenAI);
        assert_eq!(p.as_online(), Some(OnlineProvider::OpenAI));
    }

    #[test]
    fn from_online_provider() {
        let p: Provider = OnlineProvider::Mistral.into();
        assert_eq!(p, Provider::Online(OnlineProvider::Mistral));
    }

    #[test]
    fn display_round_trips() {
        for provider in [
            Provider::Local,
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
    fn api_base_urls() {
        assert!(
            OnlineProvider::OpenAI
                .api_base_url()
                .starts_with("https://")
        );
        assert!(
            OnlineProvider::Mistral
                .api_base_url()
                .starts_with("https://")
        );
        assert!(
            OnlineProvider::Deepgram
                .api_base_url()
                .starts_with("https://")
        );
    }

    #[test]
    fn default_is_local() {
        assert_eq!(Provider::default(), Provider::Local);
    }
}
