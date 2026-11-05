// SPDX-License-Identifier: GPL-3.0-only

//! The pipeline stages, each named with the feature it belongs to.
//!
//! Each feature numbers its own pipeline from 1, under its own routes: speech
//! to text's transcription and post-processing are `/stt/pipeline/1` and
//! `/stt/pipeline/2`. A bare number names a different stage in each feature,
//! so the app addresses a stage by [`Stage`], and the number appears only where
//! it meets the wire: a route, a response, an event.

use std::fmt;

use super_stt_shared::models::protocol::{POST_PROCESSOR_STAGE, TRANSCRIPTION_STAGE};

use super::ErrorScope;

/// One pipeline stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Speech to text's stage 1: speech becomes text.
    Transcription,
    /// Speech to text's stage 2: that text is rewritten before it is typed.
    PostProcessing,
}

impl Stage {
    /// Every stage, in pipeline order.
    pub const ALL: [Self; 2] = [Self::Transcription, Self::PostProcessing];

    /// The stage's number in its feature's pipeline: what its route takes and
    /// its events carry.
    #[must_use]
    pub const fn number(self) -> u32 {
        match self {
            Self::Transcription => TRANSCRIPTION_STAGE,
            Self::PostProcessing => POST_PROCESSOR_STAGE,
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
}

impl Stage {
    /// The stage as one the app reads back from the daemon, or `None` for
    /// transcription, which the app holds itself.
    #[must_use]
    pub const fn block(self) -> Option<BlockStage> {
        match self {
            Self::Transcription => None,
            Self::PostProcessing => Some(BlockStage::PostProcessing),
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
}

impl BlockStage {
    /// Every such stage, in pipeline order.
    pub const ALL: [Self; 1] = [Self::PostProcessing];

    /// The stage itself.
    #[must_use]
    pub const fn stage(self) -> Stage {
        match self {
            Self::PostProcessing => Stage::PostProcessing,
        }
    }

    /// Where a failed save of the stage is reported: on its own card.
    #[must_use]
    pub const fn error_scope(self) -> ErrorScope {
        match self {
            Self::PostProcessing => ErrorScope::PostProcessing,
        }
    }

    /// What a message calls the thing the stage runs.
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::PostProcessing => "post-processor",
        }
    }
}

/// How logs name a stage.
impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Transcription => "transcription",
            Self::PostProcessing => "post-processing",
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
        for stage in Stage::ALL {
            assert_eq!(Stage::speech_to_text(stage.number()), Some(stage));
        }
        assert_eq!(Stage::speech_to_text(3), None);
    }
}
