// SPDX-License-Identifier: GPL-3.0-only
//! The daemon's audio cues. The player is `super_engine_daemon::beeper`'s;
//! these bind it to Super TTS, so `SUPER_TTS_MUTE_CUES=1` silences every cue,
//! which is what the test daemons set.

use anyhow::Result;
use super_tts_shared::product::SUPER_TTS;

/// Play a sequence of beeps. See `super_engine_daemon::beeper::play_beep_sequence`.
///
/// # Errors
///
/// Returns an error if no output device is available or if the output stream
/// cannot be created or played.
pub fn play_beep_sequence(
    frequencies: &[f32],
    duration_ms: u64,
    fade_in_ms: u64,
    fade_out_ms: u64,
    volume: f32,
) -> Result<()> {
    super_engine_daemon::beeper::play_beep_sequence(
        &SUPER_TTS,
        frequencies,
        duration_ms,
        fade_in_ms,
        fade_out_ms,
        volume,
    )
}

/// [`play_beep_sequence`] on a blocking thread, so an async caller does not
/// stall a worker for the sound's full duration.
///
/// # Errors
///
/// Propagates the playback error, or reports a task-join failure.
pub async fn play_beep_sequence_async(
    frequencies: Vec<f32>,
    duration_ms: u64,
    fade_in_ms: u64,
    fade_out_ms: u64,
    volume: f32,
) -> Result<()> {
    super_engine_daemon::beeper::play_beep_sequence_async(
        &SUPER_TTS,
        frequencies,
        duration_ms,
        fade_in_ms,
        fade_out_ms,
        volume,
    )
    .await
}
