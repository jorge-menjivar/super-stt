// SPDX-License-Identifier: GPL-3.0-only
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use strum_macros::{AsRefStr, EnumCount, EnumIter, VariantArray, VariantNames};

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

    // Voxtral models
    #[value(name = "voxtral-small")]
    VoxtralSmall,
    #[value(name = "voxtral-mini")]
    VoxtralMini,

    // OpenAI API models (online — requires allow_online_models)
    #[value(name = "openai-whisper-1")]
    OpenAIWhisper1,
    #[value(name = "openai-gpt-4o-transcribe")]
    OpenAIGpt4oTranscribe,
    #[value(name = "openai-gpt-4o-mini-transcribe")]
    OpenAIGpt4oMiniTranscribe,

    // Mistral API models (online — requires allow_online_models)
    #[value(name = "mistral-voxtral-mini-transcribe-v2")]
    MistralVoxtralMiniTranscribeV2,

    // Deepgram API models (online — requires allow_online_models)
    #[value(name = "deepgram-nova-3")]
    DeepgramNova3,
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
            Self::OpenAIWhisper1 => write!(f, "openai-whisper-1"),
            Self::OpenAIGpt4oTranscribe => write!(f, "openai-gpt-4o-transcribe"),
            Self::OpenAIGpt4oMiniTranscribe => write!(f, "openai-gpt-4o-mini-transcribe"),
            Self::MistralVoxtralMiniTranscribeV2 => {
                write!(f, "mistral-voxtral-mini-transcribe-v2")
            }
            Self::DeepgramNova3 => write!(f, "deepgram-nova-3"),
        }
    }
}

impl STTModel {
    #[must_use]
    pub fn is_multilingual(&self) -> bool {
        match self {
            Self::WhisperTiny
            | Self::WhisperBase
            | Self::WhisperSmall
            | Self::WhisperMedium
            | Self::WhisperLarge
            | Self::WhisperLargeV2
            | Self::WhisperLargeV3
            | Self::WhisperLargeV3Turbo
            | Self::WhisperDistilLargeV2
            | Self::WhisperDistilLargeV3
            | Self::VoxtralSmall
            | Self::VoxtralMini
            | Self::OpenAIWhisper1
            | Self::OpenAIGpt4oTranscribe
            | Self::OpenAIGpt4oMiniTranscribe
            | Self::MistralVoxtralMiniTranscribeV2
            | Self::DeepgramNova3 => true,
            Self::WhisperTinyEn
            | Self::WhisperBaseEn
            | Self::WhisperSmallEn
            | Self::WhisperMediumEn
            | Self::WhisperDistilMediumEn => false,
        }
    }

    #[must_use]
    pub fn is_voxtral(&self) -> bool {
        match self {
            Self::VoxtralSmall | Self::VoxtralMini => true,
            Self::WhisperTiny
            | Self::WhisperBase
            | Self::WhisperSmall
            | Self::WhisperMedium
            | Self::WhisperLarge
            | Self::WhisperLargeV2
            | Self::WhisperLargeV3
            | Self::WhisperLargeV3Turbo
            | Self::WhisperDistilLargeV2
            | Self::WhisperDistilLargeV3
            | Self::WhisperTinyEn
            | Self::WhisperBaseEn
            | Self::WhisperSmallEn
            | Self::WhisperMediumEn
            | Self::WhisperDistilMediumEn
            | Self::OpenAIWhisper1
            | Self::OpenAIGpt4oTranscribe
            | Self::OpenAIGpt4oMiniTranscribe
            | Self::MistralVoxtralMiniTranscribeV2
            | Self::DeepgramNova3 => false,
        }
    }

    /// Returns true if this model requires an online API (audio leaves the device).
    #[must_use]
    pub fn is_online(&self) -> bool {
        matches!(
            self,
            Self::OpenAIWhisper1
                | Self::OpenAIGpt4oTranscribe
                | Self::OpenAIGpt4oMiniTranscribe
                | Self::MistralVoxtralMiniTranscribeV2
                | Self::DeepgramNova3
        )
    }

    /// Returns the API model ID string used in transcription requests.
    ///
    /// # Panics
    ///
    /// Panics if called on a non-online model.
    #[must_use]
    pub fn api_model_id(&self) -> &'static str {
        match self {
            Self::OpenAIWhisper1 => "whisper-1",
            Self::OpenAIGpt4oTranscribe => "gpt-4o-transcribe",
            Self::OpenAIGpt4oMiniTranscribe => "gpt-4o-mini-transcribe",
            Self::MistralVoxtralMiniTranscribeV2 => "voxtral-mini-latest",
            Self::DeepgramNova3 => "nova-3",
            _ => panic!("api_model_id called on non-online model: {self}"),
        }
    }

    /// Returns the provider name for keyring API key lookups (e.g. `"openai"`, `"mistral"`).
    ///
    /// # Panics
    ///
    /// Panics if called on a non-online model.
    #[must_use]
    pub fn api_provider(&self) -> &'static str {
        match self {
            Self::OpenAIWhisper1
            | Self::OpenAIGpt4oTranscribe
            | Self::OpenAIGpt4oMiniTranscribe => "openai",
            Self::MistralVoxtralMiniTranscribeV2 => "mistral",
            Self::DeepgramNova3 => "deepgram",
            _ => panic!("api_provider called on non-online model: {self}"),
        }
    }

    /// Returns the base URL for this model's API.
    ///
    /// # Panics
    ///
    /// Panics if called on a non-online model.
    #[must_use]
    pub fn api_base_url(&self) -> &'static str {
        match self {
            Self::OpenAIWhisper1
            | Self::OpenAIGpt4oTranscribe
            | Self::OpenAIGpt4oMiniTranscribe => "https://api.openai.com",
            Self::MistralVoxtralMiniTranscribeV2 => "https://api.mistral.ai",
            Self::DeepgramNova3 => "https://api.deepgram.com",
            _ => panic!("api_base_url called on non-online model: {self}"),
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
            // Online models don't have HuggingFace repos
            Self::OpenAIWhisper1 => ("openai/whisper-1", ""),
            Self::OpenAIGpt4oTranscribe => ("openai/gpt-4o-transcribe", ""),
            Self::OpenAIGpt4oMiniTranscribe => ("openai/gpt-4o-mini-transcribe", ""),
            Self::MistralVoxtralMiniTranscribeV2 => ("mistralai/voxtral-mini-transcribe-v2", ""),
            Self::DeepgramNova3 => ("deepgram/nova-3", ""),
        }
    }

    /// Get minimum processing interval for real-time transcription based on model performance characteristics
    #[must_use]
    pub fn get_processing_interval(&self) -> std::time::Duration {
        match self {
            // Fast models - can handle frequent updates
            Self::WhisperTiny | Self::WhisperTinyEn => std::time::Duration::from_millis(1000),
            Self::WhisperBase | Self::WhisperBaseEn => std::time::Duration::from_millis(1500),

            // Semi-fast models - can handle frequent updates but with a slight delay
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

            // Online models - network latency dependent
            Self::OpenAIWhisper1
            | Self::OpenAIGpt4oTranscribe
            | Self::OpenAIGpt4oMiniTranscribe
            | Self::MistralVoxtralMiniTranscribeV2
            | Self::DeepgramNova3 => std::time::Duration::from_millis(3000),
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
            "openai-whisper-1" => Ok(Self::OpenAIWhisper1),
            "openai-gpt-4o-transcribe" => Ok(Self::OpenAIGpt4oTranscribe),
            "openai-gpt-4o-mini-transcribe" => Ok(Self::OpenAIGpt4oMiniTranscribe),
            "mistral-voxtral-mini-transcribe-v2" => Ok(Self::MistralVoxtralMiniTranscribeV2),
            "deepgram-nova-3" => Ok(Self::DeepgramNova3),
            _ => Err(format!("Unknown model: {s}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_online_returns_true_for_openai_models() {
        assert!(STTModel::OpenAIWhisper1.is_online());
        assert!(STTModel::OpenAIGpt4oTranscribe.is_online());
        assert!(STTModel::OpenAIGpt4oMiniTranscribe.is_online());
    }

    #[test]
    fn is_online_returns_false_for_local_models() {
        assert!(!STTModel::WhisperTiny.is_online());
        assert!(!STTModel::WhisperLargeV3.is_online());
        assert!(!STTModel::VoxtralSmall.is_online());
        assert!(!STTModel::VoxtralMini.is_online());
        assert!(!STTModel::WhisperDistilLargeV3.is_online());
    }

    #[test]
    fn api_model_id_returns_correct_strings() {
        assert_eq!(STTModel::OpenAIWhisper1.api_model_id(), "whisper-1");
        assert_eq!(
            STTModel::OpenAIGpt4oTranscribe.api_model_id(),
            "gpt-4o-transcribe"
        );
        assert_eq!(
            STTModel::OpenAIGpt4oMiniTranscribe.api_model_id(),
            "gpt-4o-mini-transcribe"
        );
        assert_eq!(
            STTModel::MistralVoxtralMiniTranscribeV2.api_model_id(),
            "voxtral-mini-latest"
        );
    }

    #[test]
    fn api_provider_returns_correct_strings() {
        assert_eq!(STTModel::OpenAIWhisper1.api_provider(), "openai");
        assert_eq!(STTModel::OpenAIGpt4oTranscribe.api_provider(), "openai");
        assert_eq!(
            STTModel::MistralVoxtralMiniTranscribeV2.api_provider(),
            "mistral"
        );
    }

    #[test]
    fn api_base_url_returns_correct_urls() {
        assert_eq!(
            STTModel::OpenAIWhisper1.api_base_url(),
            "https://api.openai.com"
        );
        assert_eq!(
            STTModel::MistralVoxtralMiniTranscribeV2.api_base_url(),
            "https://api.mistral.ai"
        );
    }

    #[test]
    fn from_str_round_trips_for_online_models() {
        let models = [
            STTModel::OpenAIWhisper1,
            STTModel::OpenAIGpt4oTranscribe,
            STTModel::OpenAIGpt4oMiniTranscribe,
            STTModel::MistralVoxtralMiniTranscribeV2,
            STTModel::DeepgramNova3,
        ];
        for model in &models {
            let s = model.to_string();
            let parsed: STTModel = s.parse().unwrap();
            assert_eq!(*model, parsed);
        }
    }

    #[test]
    fn online_models_are_multilingual() {
        assert!(STTModel::OpenAIWhisper1.is_multilingual());
        assert!(STTModel::OpenAIGpt4oTranscribe.is_multilingual());
        assert!(STTModel::OpenAIGpt4oMiniTranscribe.is_multilingual());
        assert!(STTModel::MistralVoxtralMiniTranscribeV2.is_multilingual());
        assert!(STTModel::DeepgramNova3.is_multilingual());
    }

    #[test]
    fn online_models_are_not_voxtral() {
        assert!(!STTModel::OpenAIWhisper1.is_voxtral());
        assert!(!STTModel::OpenAIGpt4oTranscribe.is_voxtral());
        assert!(!STTModel::OpenAIGpt4oMiniTranscribe.is_voxtral());
        assert!(!STTModel::MistralVoxtralMiniTranscribeV2.is_voxtral());
        assert!(!STTModel::DeepgramNova3.is_voxtral());
    }

    #[test]
    fn is_online_returns_true_for_mistral_models() {
        assert!(STTModel::MistralVoxtralMiniTranscribeV2.is_online());
    }

    #[test]
    fn is_online_returns_true_for_deepgram_models() {
        assert!(STTModel::DeepgramNova3.is_online());
    }

    #[test]
    fn deepgram_api_methods() {
        assert_eq!(STTModel::DeepgramNova3.api_model_id(), "nova-3");
        assert_eq!(STTModel::DeepgramNova3.api_provider(), "deepgram");
        assert_eq!(
            STTModel::DeepgramNova3.api_base_url(),
            "https://api.deepgram.com"
        );
    }

    #[test]
    fn default_model_is_not_online() {
        assert!(!STTModel::default().is_online());
    }

    #[test]
    #[should_panic(expected = "api_model_id called on non-online model")]
    fn api_model_id_panics_for_local_model() {
        let _ = STTModel::WhisperTiny.api_model_id();
    }

    #[test]
    #[should_panic(expected = "api_provider called on non-online model")]
    fn api_provider_panics_for_local_model() {
        let _ = STTModel::WhisperTiny.api_provider();
    }

    #[test]
    #[should_panic(expected = "api_base_url called on non-online model")]
    fn api_base_url_panics_for_local_model() {
        let _ = STTModel::WhisperTiny.api_base_url();
    }

    #[test]
    fn all_online_models_have_consistent_api_methods() {
        use strum::VariantArray;
        for model in STTModel::VARIANTS {
            if model.is_online() {
                // These should not panic for any online model
                let _ = model.api_model_id();
                let _ = model.api_provider();
                let url = model.api_base_url();
                assert!(
                    url.starts_with("https://"),
                    "{model}: api_base_url should start with https://"
                );
            }
        }
    }

    #[test]
    fn no_local_model_is_online() {
        use strum::VariantArray;
        for model in STTModel::VARIANTS {
            if !model.is_online() {
                // Local models should be either whisper or voxtral
                let name = model.to_string();
                assert!(
                    name.starts_with("whisper-") || name.starts_with("voxtral-"),
                    "{model}: unexpected local model prefix"
                );
            }
        }
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
}
