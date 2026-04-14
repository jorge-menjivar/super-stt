// SPDX-License-Identifier: GPL-3.0-only
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use strum_macros::{AsRefStr, EnumCount, EnumIter, VariantArray, VariantNames};

use super::provider::{OnlineProvider, Provider};

#[derive(
    Serialize,
    Deserialize,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    ValueEnum,
    EnumIter,
    EnumCount,
    VariantArray,
    VariantNames,
    AsRefStr,
)]
pub enum STTModel {
    #[value(name = "whisper-tiny")]
    #[default]
    WhisperTiny,
    #[value(name = "whisper-tiny.en")]
    WhisperTinyEn,
    #[value(name = "whisper-base")]
    WhisperBase,
    #[value(name = "whisper-base.en")]
    WhisperBaseEn,
    #[value(name = "whisper-small")]
    WhisperSmall,
    #[value(name = "whisper-small.en")]
    WhisperSmallEn,
    #[value(name = "whisper-medium")]
    WhisperMedium,
    #[value(name = "whisper-medium.en")]
    WhisperMediumEn,
    #[value(name = "whisper-large")]
    WhisperLarge,
    #[value(name = "whisper-large-v2")]
    WhisperLargeV2,
    #[value(name = "whisper-large-v3")]
    WhisperLargeV3,
    #[value(name = "whisper-large-v3-turbo")]
    WhisperLargeV3Turbo,
    #[value(name = "whisper-distil-medium.en")]
    WhisperDistilMediumEn,
    #[value(name = "whisper-distil-large-v2")]
    WhisperDistilLargeV2,
    #[value(name = "whisper-distil-large-v3")]
    WhisperDistilLargeV3,

    // Voxtral models (local + Mistral API)
    #[value(name = "voxtral-small")]
    VoxtralSmall,
    #[value(name = "voxtral-mini")]
    VoxtralMini,

    // Online-only models
    #[value(name = "whisper-1")]
    Whisper1,
    #[value(name = "gpt-4o-transcribe")]
    Gpt4oTranscribe,
    #[value(name = "gpt-4o-mini-transcribe")]
    Gpt4oMiniTranscribe,
    #[value(name = "voxtral-mini-transcribe-v2")]
    VoxtralMiniTranscribeV2,
    #[value(name = "nova-3")]
    Nova3,
}

impl std::fmt::Display for STTModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WhisperTiny => write!(f, "whisper-tiny"),
            Self::WhisperTinyEn => write!(f, "whisper-tiny.en"),
            Self::WhisperBase => write!(f, "whisper-base"),
            Self::WhisperBaseEn => write!(f, "whisper-base.en"),
            Self::WhisperSmall => write!(f, "whisper-small"),
            Self::WhisperSmallEn => write!(f, "whisper-small.en"),
            Self::WhisperMedium => write!(f, "whisper-medium"),
            Self::WhisperMediumEn => write!(f, "whisper-medium.en"),
            Self::WhisperLarge => write!(f, "whisper-large"),
            Self::WhisperLargeV2 => write!(f, "whisper-large-v2"),
            Self::WhisperLargeV3 => write!(f, "whisper-large-v3"),
            Self::WhisperLargeV3Turbo => write!(f, "whisper-large-v3-turbo"),
            Self::WhisperDistilMediumEn => write!(f, "whisper-distil-medium.en"),
            Self::WhisperDistilLargeV2 => write!(f, "whisper-distil-large-v2"),
            Self::WhisperDistilLargeV3 => write!(f, "whisper-distil-large-v3"),
            Self::VoxtralSmall => write!(f, "voxtral-small"),
            Self::VoxtralMini => write!(f, "voxtral-mini"),
            Self::Whisper1 => write!(f, "whisper-1"),
            Self::Gpt4oTranscribe => write!(f, "gpt-4o-transcribe"),
            Self::Gpt4oMiniTranscribe => write!(f, "gpt-4o-mini-transcribe"),
            Self::VoxtralMiniTranscribeV2 => write!(f, "voxtral-mini-transcribe-v2"),
            Self::Nova3 => write!(f, "nova-3"),
        }
    }
}

impl STTModel {
    /// Returns the providers that support this model.
    /// The first provider in the list is the default.
    #[must_use]
    pub fn providers(&self) -> &[Provider] {
        match self {
            // Local-only whisper models
            Self::WhisperTiny
            | Self::WhisperTinyEn
            | Self::WhisperBase
            | Self::WhisperBaseEn
            | Self::WhisperSmall
            | Self::WhisperSmallEn
            | Self::WhisperMedium
            | Self::WhisperMediumEn
            | Self::WhisperLarge
            | Self::WhisperLargeV2
            | Self::WhisperLargeV3
            | Self::WhisperLargeV3Turbo
            | Self::WhisperDistilMediumEn
            | Self::WhisperDistilLargeV2
            | Self::WhisperDistilLargeV3 => &[Provider::Local],

            // Voxtral models available locally and via Mistral API
            Self::VoxtralSmall | Self::VoxtralMini => {
                &[Provider::Local, Provider::Online(OnlineProvider::Mistral)]
            }

            // OpenAI-only models
            Self::Whisper1 | Self::Gpt4oTranscribe | Self::Gpt4oMiniTranscribe => {
                &[Provider::Online(OnlineProvider::OpenAI)]
            }

            // Mistral-only online model
            Self::VoxtralMiniTranscribeV2 => &[Provider::Online(OnlineProvider::Mistral)],

            // Deepgram-only model
            Self::Nova3 => &[Provider::Online(OnlineProvider::Deepgram)],
        }
    }

    /// Returns the default provider for this model (first in the providers list).
    #[must_use]
    pub fn default_provider(&self) -> Provider {
        self.providers()[0]
    }

    /// Returns the API model ID string for a given online provider, or `None`
    /// if this model is not available via that provider.
    #[must_use]
    pub fn api_model_id(&self, provider: OnlineProvider) -> Option<&'static str> {
        match (self, provider) {
            (Self::Whisper1, OnlineProvider::OpenAI) => Some("whisper-1"),
            (Self::Gpt4oTranscribe, OnlineProvider::OpenAI) => Some("gpt-4o-transcribe"),
            (Self::Gpt4oMiniTranscribe, OnlineProvider::OpenAI) => Some("gpt-4o-mini-transcribe"),
            (Self::VoxtralMini | Self::VoxtralMiniTranscribeV2, OnlineProvider::Mistral) => {
                Some("voxtral-mini-latest")
            }
            (Self::VoxtralSmall, OnlineProvider::Mistral) => Some("voxtral-small-latest"),
            (Self::Nova3, OnlineProvider::Deepgram) => Some("nova-3"),
            _ => None,
        }
    }

    /// Whether this model requires GPU acceleration for local inference.
    /// Voxtral models need GPU; whisper models can run on CPU.
    #[must_use]
    pub fn requires_gpu(&self) -> bool {
        self.is_voxtral()
    }

    #[must_use]
    pub fn is_multilingual(&self) -> bool {
        !matches!(
            self,
            Self::WhisperTinyEn
                | Self::WhisperBaseEn
                | Self::WhisperSmallEn
                | Self::WhisperMediumEn
                | Self::WhisperDistilMediumEn
        )
    }

    #[must_use]
    pub fn is_voxtral(&self) -> bool {
        matches!(self, Self::VoxtralSmall | Self::VoxtralMini)
    }

    /// Estimated GPU VRAM required to load this model, in bytes.
    /// Returns 0 for online-only models (they don't use local GPU).
    /// These are conservative estimates that include model weights plus
    /// overhead for KV-cache, activations, and CUDA context.
    #[must_use]
    pub fn estimated_vram_bytes(&self) -> u64 {
        const GB: u64 = 1_073_741_824;
        const MB: u64 = 1_048_576;
        match self {
            // Whisper models (weights loaded in f32)
            Self::WhisperTiny | Self::WhisperTinyEn => 250 * MB,
            Self::WhisperBase | Self::WhisperBaseEn => 500 * MB,
            Self::WhisperSmall | Self::WhisperSmallEn => GB,
            Self::WhisperMedium | Self::WhisperMediumEn | Self::WhisperDistilMediumEn => 2 * GB,
            Self::WhisperLarge | Self::WhisperLargeV2 | Self::WhisperLargeV3 => 4 * GB,
            Self::WhisperLargeV3Turbo | Self::WhisperDistilLargeV2 | Self::WhisperDistilLargeV3 => {
                3 * GB
            }
            // Voxtral models
            Self::VoxtralMini => 8 * GB,
            Self::VoxtralSmall => 50 * GB,
            // Online-only models don't use local VRAM
            Self::Whisper1
            | Self::Gpt4oTranscribe
            | Self::Gpt4oMiniTranscribe
            | Self::VoxtralMiniTranscribeV2
            | Self::Nova3 => 0,
        }
    }

    #[must_use]
    pub fn model_and_revision(&self) -> (&'static str, &'static str) {
        match self {
            Self::WhisperTiny => ("openai/whisper-tiny", "main"),
            Self::WhisperTinyEn => ("openai/whisper-tiny.en", "main"),
            Self::WhisperBase => ("openai/whisper-base", "main"),
            Self::WhisperBaseEn => ("openai/whisper-base.en", "main"),
            Self::WhisperSmall => ("openai/whisper-small", "main"),
            Self::WhisperSmallEn => ("openai/whisper-small.en", "main"),
            Self::WhisperMedium => ("openai/whisper-medium", "main"),
            Self::WhisperMediumEn => ("openai/whisper-medium.en", "main"),
            Self::WhisperLarge => ("openai/whisper-large", "main"),
            Self::WhisperLargeV2 => ("openai/whisper-large-v2", "main"),
            Self::WhisperLargeV3 => ("openai/whisper-large-v3", "main"),
            Self::WhisperLargeV3Turbo => ("openai/whisper-large-v3-turbo", "main"),
            Self::WhisperDistilMediumEn => ("distil-whisper/distil-medium.en", "main"),
            Self::WhisperDistilLargeV2 => ("distil-whisper/distil-large-v2", "main"),
            Self::WhisperDistilLargeV3 => ("distil-whisper/distil-large-v3", "main"),
            Self::VoxtralSmall => ("mistralai/Voxtral-Small-24B-2507", "main"),
            Self::VoxtralMini => ("mistralai/Voxtral-Mini-3B-2507", "main"),
            // Online-only models don't have HuggingFace repos
            Self::Whisper1 => ("openai/whisper-1", ""),
            Self::Gpt4oTranscribe => ("openai/gpt-4o-transcribe", ""),
            Self::Gpt4oMiniTranscribe => ("openai/gpt-4o-mini-transcribe", ""),
            Self::VoxtralMiniTranscribeV2 => ("mistralai/voxtral-mini-transcribe-v2", ""),
            Self::Nova3 => ("deepgram/nova-3", ""),
        }
    }

    /// Get minimum processing interval for real-time transcription based on model performance characteristics
    #[must_use]
    pub fn get_processing_interval(&self) -> std::time::Duration {
        match self {
            // Fast models and online models
            Self::WhisperTiny
            | Self::WhisperTinyEn
            | Self::Whisper1
            | Self::Gpt4oTranscribe
            | Self::Gpt4oMiniTranscribe
            | Self::VoxtralMiniTranscribeV2
            | Self::Nova3 => std::time::Duration::from_millis(1000),
            Self::WhisperBase | Self::WhisperBaseEn => std::time::Duration::from_millis(1500),

            // Semi-fast models
            Self::VoxtralMini
            | Self::WhisperSmall
            | Self::WhisperSmallEn
            | Self::WhisperDistilMediumEn
            | Self::WhisperMedium
            | Self::WhisperMediumEn => std::time::Duration::from_millis(2000),
            Self::WhisperDistilLargeV2 | Self::WhisperDistilLargeV3 => {
                std::time::Duration::from_millis(2000)
            }
            Self::VoxtralSmall | Self::WhisperLargeV3Turbo => {
                std::time::Duration::from_millis(3000)
            }

            // Large models - conservative intervals
            Self::WhisperLarge | Self::WhisperLargeV2 | Self::WhisperLargeV3 => {
                std::time::Duration::from_millis(5000)
            }
        }
    }
}

impl FromStr for STTModel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "whisper-tiny" => Ok(Self::WhisperTiny),
            "whisper-tiny.en" => Ok(Self::WhisperTinyEn),
            "whisper-base" => Ok(Self::WhisperBase),
            "whisper-base.en" => Ok(Self::WhisperBaseEn),
            "whisper-small" => Ok(Self::WhisperSmall),
            "whisper-small.en" => Ok(Self::WhisperSmallEn),
            "whisper-medium" => Ok(Self::WhisperMedium),
            "whisper-medium.en" => Ok(Self::WhisperMediumEn),
            "whisper-large" => Ok(Self::WhisperLarge),
            "whisper-large-v2" => Ok(Self::WhisperLargeV2),
            "whisper-large-v3" => Ok(Self::WhisperLargeV3),
            "whisper-large-v3-turbo" => Ok(Self::WhisperLargeV3Turbo),
            "whisper-distil-medium.en" => Ok(Self::WhisperDistilMediumEn),
            "whisper-distil-large-v2" => Ok(Self::WhisperDistilLargeV2),
            "whisper-distil-large-v3" => Ok(Self::WhisperDistilLargeV3),
            "voxtral-small" => Ok(Self::VoxtralSmall),
            "voxtral-mini" => Ok(Self::VoxtralMini),
            "whisper-1" => Ok(Self::Whisper1),
            "gpt-4o-transcribe" => Ok(Self::Gpt4oTranscribe),
            "gpt-4o-mini-transcribe" => Ok(Self::Gpt4oMiniTranscribe),
            "voxtral-mini-transcribe-v2" => Ok(Self::VoxtralMiniTranscribeV2),
            "nova-3" => Ok(Self::Nova3),
            _ => Err(format!("Unknown model: {s}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_local_models() {
        assert_eq!(STTModel::WhisperTiny.providers(), &[Provider::Local]);
        assert_eq!(STTModel::WhisperLargeV3.providers(), &[Provider::Local]);
    }

    #[test]
    fn providers_voxtral_has_local_and_mistral() {
        let providers = STTModel::VoxtralMini.providers();
        assert!(providers.contains(&Provider::Local));
        assert!(providers.contains(&Provider::Online(OnlineProvider::Mistral)));
    }

    #[test]
    fn providers_online_only_models() {
        assert_eq!(
            STTModel::Whisper1.providers(),
            &[Provider::Online(OnlineProvider::OpenAI)]
        );
        assert_eq!(
            STTModel::Gpt4oTranscribe.providers(),
            &[Provider::Online(OnlineProvider::OpenAI)]
        );
        assert_eq!(
            STTModel::VoxtralMiniTranscribeV2.providers(),
            &[Provider::Online(OnlineProvider::Mistral)]
        );
        assert_eq!(
            STTModel::Nova3.providers(),
            &[Provider::Online(OnlineProvider::Deepgram)]
        );
    }

    #[test]
    fn default_provider_is_local_when_available() {
        assert_eq!(STTModel::WhisperTiny.default_provider(), Provider::Local);
        assert_eq!(STTModel::VoxtralMini.default_provider(), Provider::Local);
    }

    #[test]
    fn default_provider_online_only() {
        assert_eq!(
            STTModel::Whisper1.default_provider(),
            Provider::Online(OnlineProvider::OpenAI)
        );
        assert_eq!(
            STTModel::Nova3.default_provider(),
            Provider::Online(OnlineProvider::Deepgram)
        );
    }

    #[test]
    fn api_model_id_returns_correct_strings() {
        assert_eq!(
            STTModel::Whisper1.api_model_id(OnlineProvider::OpenAI),
            Some("whisper-1")
        );
        assert_eq!(
            STTModel::Gpt4oTranscribe.api_model_id(OnlineProvider::OpenAI),
            Some("gpt-4o-transcribe")
        );
        assert_eq!(
            STTModel::Gpt4oMiniTranscribe.api_model_id(OnlineProvider::OpenAI),
            Some("gpt-4o-mini-transcribe")
        );
        assert_eq!(
            STTModel::VoxtralMiniTranscribeV2.api_model_id(OnlineProvider::Mistral),
            Some("voxtral-mini-latest")
        );
        assert_eq!(
            STTModel::Nova3.api_model_id(OnlineProvider::Deepgram),
            Some("nova-3")
        );
    }

    #[test]
    fn api_model_id_voxtral_via_mistral() {
        assert_eq!(
            STTModel::VoxtralMini.api_model_id(OnlineProvider::Mistral),
            Some("voxtral-mini-latest")
        );
    }

    #[test]
    fn api_model_id_returns_none_for_wrong_provider() {
        assert_eq!(
            STTModel::WhisperTiny.api_model_id(OnlineProvider::OpenAI),
            None
        );
        assert_eq!(
            STTModel::Whisper1.api_model_id(OnlineProvider::Mistral),
            None
        );
    }

    #[test]
    fn is_voxtral() {
        assert!(STTModel::VoxtralSmall.is_voxtral());
        assert!(STTModel::VoxtralMini.is_voxtral());
        assert!(!STTModel::WhisperTiny.is_voxtral());
        assert!(!STTModel::Whisper1.is_voxtral());
    }

    #[test]
    fn default_model_is_local() {
        assert_eq!(STTModel::default().default_provider(), Provider::Local);
    }

    #[test]
    fn every_model_round_trips_through_display_and_from_str() {
        use strum::VariantArray;
        for model in STTModel::VARIANTS {
            let s = model.to_string();
            let parsed: STTModel = s.parse().unwrap_or_else(|e| {
                panic!("{model}: FromStr failed for '{s}': {e}");
            });
            assert_eq!(*model, parsed);
        }
    }

    #[test]
    fn every_model_has_at_least_one_provider() {
        use strum::VariantArray;
        for model in STTModel::VARIANTS {
            assert!(!model.providers().is_empty(), "{model}: has no providers");
        }
    }

    #[test]
    fn online_only_models_have_api_model_id() {
        use strum::VariantArray;
        for model in STTModel::VARIANTS {
            for provider in model.providers() {
                if let Provider::Online(op) = provider {
                    assert!(
                        model.api_model_id(*op).is_some(),
                        "{model} with {provider}: missing api_model_id"
                    );
                }
            }
        }
    }
}
