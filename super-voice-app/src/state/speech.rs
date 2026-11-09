// SPDX-License-Identifier: GPL-3.0-only
//! The Speech tab: what the daemon is saying, and the test utterance.

/// Whether the daemon is speaking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SpeakingStatus {
    #[default]
    Idle,
    Speaking,
}

/// What the Speech tab shows.
///
/// Its own state rather than the recording meter's: the microphone and the
/// speaker are two devices, and one can be busy while the other is not.
#[derive(Debug, Clone, Default)]
pub struct SpeechState {
    /// Driven only by the daemon's `speaking_state` events, because the daemon
    /// speaks for whoever asked.
    pub status: SpeakingStatus,
    /// The utterance the daemon says it is speaking, when it names one.
    pub utterance: Option<String>,
    /// What the test field holds.
    pub test_text: String,
    /// The playback level, for the meter.
    pub level: f32,
    /// Whether audio is reaching the speaker. An accepted utterance is
    /// speaking before its first sample plays.
    pub is_playing: bool,
}
