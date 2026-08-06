// SPDX-License-Identifier: GPL-3.0-only

use std::collections::VecDeque;
use std::time::{Duration, Instant};

// Audio recording configuration constants
pub const GRACE_PERIOD: Duration = Duration::from_secs(2);
pub const SILENCE_TIMEOUT: Duration = Duration::from_millis(1500);
pub const NO_SPEECH_TIMEOUT: Duration = Duration::from_secs(5);

// Buffers and adaptive thresholds
pub const SPEECH_BUFFER_SIZE: usize = 5;
pub const RECENT_LEVELS_BUFFER_SIZE: usize = 200;
pub const QUIET_LEVELS_BUFFER_SIZE: usize = 100;
pub const ACTIVE_LEVELS_BUFFER_SIZE: usize = 100;

pub const DEFAULT_BASELINE_LEVEL: f32 = 0.005;
pub const DEFAULT_ACTIVE_LEVEL: f32 = 0.015;
pub const MIN_SPEECH_THRESHOLD: f32 = 0.003;
pub const MAX_SPEECH_THRESHOLD: f32 = 0.025;
pub const THRESHOLD_CONTRAST_FRACTION: f32 = 0.3;
pub const MIN_ACTIVE_BOOST: f32 = 0.003;
pub const BASELINE_PERCENTILE: f32 = 0.75;
pub const ACTIVE_PERCENTILE: f32 = 0.25;
pub const SPEECH_DETECTION_THRESHOLD: f32 = 0.2;

#[derive(Debug, Clone)]
pub struct RecordingState {
    pub recording: bool,
    pub silence_start: Option<Instant>,
    pub stop_requested: bool,
    pub speech_buffer: VecDeque<bool>,
    pub recording_start: Option<Instant>,
    /// When true, silence detection is disabled; recording stops only via external signal.
    pub silence_detection_disabled: bool,

    pub recent_levels: VecDeque<f32>,
    pub quiet_levels: VecDeque<f32>,
    pub active_levels: VecDeque<f32>,
    pub baseline_level: f32,
    pub active_level: f32,
}

impl Default for RecordingState {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            recording: false,
            silence_start: None,
            stop_requested: false,
            speech_buffer: VecDeque::with_capacity(SPEECH_BUFFER_SIZE),
            recording_start: None,
            silence_detection_disabled: false,

            recent_levels: VecDeque::with_capacity(RECENT_LEVELS_BUFFER_SIZE),
            quiet_levels: VecDeque::with_capacity(QUIET_LEVELS_BUFFER_SIZE),
            active_levels: VecDeque::with_capacity(ACTIVE_LEVELS_BUFFER_SIZE),
            baseline_level: DEFAULT_BASELINE_LEVEL,
            active_level: DEFAULT_ACTIVE_LEVEL,
        }
    }

    #[must_use]
    pub fn should_stop(&self) -> bool {
        self.stop_requested
    }

    pub fn update_adaptive_levels(&mut self, rms: f32, is_currently_active: bool) {
        // Drop non-finite samples before they reach the level buffers. An empty
        // capture chunk computes `0.0/0.0 = NaN`, and a misbehaving device can
        // deliver NaN samples; a NaN in the buffers would poison the percentile
        // sorts in `update_level_estimates` and panic the capture thread.
        if !rms.is_finite() {
            return;
        }

        if self.recent_levels.len() >= RECENT_LEVELS_BUFFER_SIZE {
            self.recent_levels.pop_front();
        }
        self.recent_levels.push_back(rms);

        if is_currently_active {
            if self.active_levels.len() >= ACTIVE_LEVELS_BUFFER_SIZE {
                self.active_levels.pop_front();
            }
            self.active_levels.push_back(rms);
        } else {
            if self.quiet_levels.len() >= QUIET_LEVELS_BUFFER_SIZE {
                self.quiet_levels.pop_front();
            }
            self.quiet_levels.push_back(rms);
        }

        self.update_level_estimates();
    }

    fn update_level_estimates(&mut self) {
        if !self.quiet_levels.is_empty() {
            let mut sorted_quiet: Vec<f32> = self.quiet_levels.iter().copied().collect();
            // `total_cmp` is a total order over f32 (NaN-safe), so the sort can
            // never panic even if a non-finite value slips past the guard above.
            sorted_quiet.sort_by(f32::total_cmp);
            // sorted_quiet.len() ≤ QUIET_LEVELS_BUFFER_SIZE = 100; fits u32 and f32 exactly.
            let len_f32 =
                crate::num_cast::u32_to_f32(u32::try_from(sorted_quiet.len()).unwrap_or(u32::MAX));
            // Product is ≤ 100.0; truncation to usize is intentional percentile indexing.
            let percentile_index = crate::num_cast::f32_to_usize(len_f32 * BASELINE_PERCENTILE);
            self.baseline_level = sorted_quiet
                .get(percentile_index)
                .copied()
                .unwrap_or(self.baseline_level);
        }

        if !self.active_levels.is_empty() {
            let mut sorted_active: Vec<f32> = self.active_levels.iter().copied().collect();
            sorted_active.sort_by(f32::total_cmp);
            // sorted_active.len() ≤ ACTIVE_LEVELS_BUFFER_SIZE = 100; fits u32 and f32 exactly.
            let len_f32 =
                crate::num_cast::u32_to_f32(u32::try_from(sorted_active.len()).unwrap_or(u32::MAX));
            // Product is ≤ 100.0; truncation to usize is intentional percentile indexing.
            let percentile_index = crate::num_cast::f32_to_usize(len_f32 * ACTIVE_PERCENTILE);
            self.active_level = sorted_active
                .get(percentile_index)
                .copied()
                .unwrap_or(self.active_level);
        }

        if self.active_level <= self.baseline_level {
            self.active_level = self.baseline_level + MIN_ACTIVE_BOOST;
        }
    }

    #[must_use]
    pub fn get_speech_threshold(&self) -> f32 {
        let contrast = self.active_level - self.baseline_level;
        let threshold = self.baseline_level + (contrast * THRESHOLD_CONTRAST_FRACTION);
        threshold.clamp(MIN_SPEECH_THRESHOLD, MAX_SPEECH_THRESHOLD)
    }

    pub fn add_speech_decision(&mut self, is_speech: bool) -> bool {
        if self.speech_buffer.len() >= SPEECH_BUFFER_SIZE {
            self.speech_buffer.pop_front();
        }
        self.speech_buffer.push_back(is_speech);

        let speech_count = self.speech_buffer.iter().filter(|&&x| x).count();
        let total_count = self.speech_buffer.len();
        // speech_count and total_count are tiny (≤ SPEECH_BUFFER_SIZE = 5); fit in u32 exactly.
        let speech_f32 =
            crate::num_cast::u32_to_f32(u32::try_from(speech_count).unwrap_or(u32::MAX));
        let total_f32 = crate::num_cast::u32_to_f32(u32::try_from(total_count).unwrap_or(u32::MAX));
        speech_f32 / total_f32 > SPEECH_DETECTION_THRESHOLD
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::processing::process_audio_samples;
    use parking_lot::Mutex;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    /// A non-finite RMS (an empty capture buffer computes `0.0/0.0 = NaN`, and a
    /// misbehaving device can deliver NaN samples) must not reach the percentile
    /// sort in `update_level_estimates`, whose `partial_cmp(...).unwrap()` would
    /// panic on the cpal capture-callback thread. The level estimates must also
    /// stay finite (a NaN must not poison the adaptive thresholds).
    #[test]
    fn non_finite_rms_does_not_panic_or_poison_levels() {
        let mut state = RecordingState::new();
        // Prime both buffers with finite levels so the sort has >1 element.
        state.update_adaptive_levels(0.01, false);
        state.update_adaptive_levels(0.02, true);
        // These would panic under `partial_cmp(...).unwrap()`.
        state.update_adaptive_levels(f32::NAN, false);
        state.update_adaptive_levels(f32::NAN, true);
        state.update_adaptive_levels(f32::INFINITY, false);
        assert!(state.baseline_level.is_finite(), "baseline poisoned by NaN");
        assert!(
            state.active_level.is_finite(),
            "active level poisoned by NaN"
        );
    }

    /// Drive `process_audio_samples` over `chunks` chunks of constant
    /// amplitude and report whether the speech latch ever flipped.
    fn latch_after_chunks(amplitude: f32, chunks: usize) -> bool {
        let buffer = Arc::new(Mutex::new(VecDeque::new()));
        let state = Arc::new(Mutex::new(RecordingState::new()));
        let (level_tx, _rx) = broadcast::channel(64);

        let samples = vec![amplitude; 480];
        for _ in 0..chunks {
            process_audio_samples(&samples, &buffer, &state, &level_tx);
        }

        let flipped = state.lock().recording;
        flipped
    }

    /// The whole silence gate rests on this: a take with no speech must leave
    /// `recording` false, so the daemon can skip transcription.
    #[test]
    fn latch_stays_false_through_pure_silence() {
        assert!(
            !latch_after_chunks(0.0, 20),
            "silence must never flip the speech latch"
        );
    }

    /// And the converse — real audio must flip it, or the gate would swallow
    /// genuine speech.
    #[test]
    fn latch_flips_on_speech_level_audio() {
        assert!(
            latch_after_chunks(0.5, 20),
            "speech-level audio must flip the speech latch"
        );
    }

    /// The latch is one-way within a take: speech followed by trailing silence
    /// still reports that speech happened.
    #[test]
    fn latch_stays_true_after_speech_then_silence() {
        let buffer = Arc::new(Mutex::new(VecDeque::new()));
        let state = Arc::new(Mutex::new(RecordingState::new()));
        let (level_tx, _rx) = broadcast::channel(64);

        let loud = vec![0.5f32; 480];
        for _ in 0..5 {
            process_audio_samples(&loud, &buffer, &state, &level_tx);
        }
        let quiet = vec![0.0f32; 480];
        for _ in 0..20 {
            process_audio_samples(&quiet, &buffer, &state, &level_tx);
        }

        assert!(
            state.lock().recording,
            "trailing silence must not clear the latch"
        );
    }
}
