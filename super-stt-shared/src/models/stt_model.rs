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
            | Self::VoxtralMini => true,
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
            | Self::WhisperDistilMediumEn => false,
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
            _ => Err(format!("Unknown model: {s}")),
        }
    }
}
