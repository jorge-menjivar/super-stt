// SPDX-License-Identifier: GPL-3.0-only

//! The pipeline stages, each named with the feature it belongs to.
//!
//! Each feature numbers its own pipeline from 1, under its own routes: speech
//! to text's transcription and post-processing are `/stt/pipeline/1` and
//! `/stt/pipeline/2`, and text to speech's synthesis is `/tts/pipeline/1`. A bare number names a different stage in each feature,
//! so the app addresses a stage by [`Stage`], and the number appears only where
//! it meets the wire: a route, a response, an event.

use std::fmt;

use super_voice_shared::models::protocol::{POST_PROCESSOR_STAGE, TRANSCRIPTION_STAGE};
use super_voice_shared::tts::models::protocol::SYNTHESIS_STAGE;

use super::ErrorScope;

/// One pipeline stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Speech to text's stage 1: speech becomes text.
    Transcription,
    /// Speech to text's stage 2: that text is rewritten before it is typed.
    PostProcessing,
    /// Text to speech's stage 1: text becomes speech.
    Synthesis,
}

impl Stage {
    /// Every stage, in pipeline order.
    pub const ALL: [Self; 3] = [Self::Transcription, Self::PostProcessing, Self::Synthesis];

    /// The stage's number in its feature's pipeline: what its route takes and
    /// its events carry.
    #[must_use]
    pub const fn number(self) -> u32 {
        match self {
            Self::Transcription => TRANSCRIPTION_STAGE,
            Self::PostProcessing => POST_PROCESSOR_STAGE,
            Self::Synthesis => SYNTHESIS_STAGE,
        }
    }

    /// What the stage's routes are built from: its feature's prefix and its
    /// number, so `/{feature}/pipeline/{number}` is `/stt/pipeline/1`.
    ///
    /// The two parts rather than the built path, so each route stays one
    /// literal at its call site, where the path contract test reads it.
    #[must_use]
    pub const fn route(self) -> (&'static str, u32) {
        (self.feature(), self.number())
    }

    /// The feature's route prefix.
    const fn feature(self) -> &'static str {
        match self {
            Self::Transcription | Self::PostProcessing => "stt",
            Self::Synthesis => "tts",
        }
    }

    /// The speech-to-text stage a response or an event numbers, or `None` for
    /// a number that pipeline does not have.
    #[must_use]
    pub const fn speech_to_text(number: u32) -> Option<Self> {
        match number {
            TRANSCRIPTION_STAGE => Some(Self::Transcription),
            POST_PROCESSOR_STAGE => Some(Self::PostProcessing),
            _ => None,
        }
    }

    /// The stage an event names: its `feature`, then its number in that
    /// feature's pipeline. An event naming no feature is speech to text's, as
    /// every event was before text to speech joined the daemon.
    #[must_use]
    pub fn from_wire(feature: Option<&str>, number: u32) -> Option<Self> {
        match feature {
            None | Some("stt") => Self::speech_to_text(number),
            Some("tts") if number == SYNTHESIS_STAGE => Some(Self::Synthesis),
            Some(_) => None,
        }
    }
}

impl Stage {
    /// The stage as one the app reads back from the daemon, or `None` for
    /// transcription, which the app holds itself.
    #[must_use]
    pub const fn block(self) -> Option<BlockStage> {
        match self {
            Self::Transcription => None,
            Self::PostProcessing => Some(BlockStage::PostProcessing),
            Self::Synthesis => Some(BlockStage::Synthesis),
        }
    }
}

/// A stage the app reads back from the daemon after every change, rather than
/// holding its selection and running model itself.
///
/// Transcription keeps its own, so its card can move before the daemon
/// answers. Every other stage re-reads its block (`GET
/// /{feature}/pipeline/{number}` and its model) instead, with one
/// implementation for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockStage {
    PostProcessing,
    Synthesis,
}

impl BlockStage {
    /// Every such stage, in pipeline order.
    pub const ALL: [Self; 2] = [Self::PostProcessing, Self::Synthesis];

    /// The stage itself.
    #[must_use]
    pub const fn stage(self) -> Stage {
        match self {
            Self::PostProcessing => Stage::PostProcessing,
            Self::Synthesis => Stage::Synthesis,
        }
    }

    /// Where a failed save of the stage is reported: on its own card.
    #[must_use]
    pub const fn error_scope(self) -> ErrorScope {
        match self {
            Self::PostProcessing => ErrorScope::PostProcessing,
            Self::Synthesis => ErrorScope::Synthesis,
        }
    }

    /// What a message calls the thing the stage runs.
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::PostProcessing => "post-processor",
            Self::Synthesis => "speech model",
        }
    }
}

/// How logs name a stage.
impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Transcription => "transcription",
            Self::PostProcessing => "post-processing",
            Self::Synthesis => "synthesis",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Stage;

    #[test]
    fn each_stage_answers_on_its_features_route() {
        assert_eq!(Stage::Transcription.route(), ("stt", 1));
        assert_eq!(Stage::PostProcessing.route(), ("stt", 2));
        assert_eq!(Stage::Synthesis.route(), ("tts", 1));
    }

    #[test]
    fn every_stage_but_transcription_is_read_back() {
        for stage in Stage::ALL {
            assert_eq!(
                stage.block().map(super::BlockStage::stage),
                (stage != Stage::Transcription).then_some(stage)
            );
        }
    }

    #[test]
    fn a_number_names_the_stage_it_carries() {
        for stage in [Stage::Transcription, Stage::PostProcessing] {
            assert_eq!(Stage::speech_to_text(stage.number()), Some(stage));
        }
        assert_eq!(Stage::speech_to_text(3), None);
    }

    /// An event names its feature; one that names none is speech to text's.
    #[test]
    fn an_event_names_its_stage_by_feature_and_number() {
        for stage in Stage::ALL {
            let (feature, number) = stage.route();
            assert_eq!(Stage::from_wire(Some(feature), number), Some(stage));
        }
        assert_eq!(Stage::from_wire(None, 1), Some(Stage::Transcription));
        assert_eq!(Stage::from_wire(Some("tts"), 2), None);
        assert_eq!(Stage::from_wire(Some("ocr"), 1), None);
    }
}
