// SPDX-License-Identifier: GPL-3.0-only

use std::collections::VecDeque;
use std::time::{Duration, Instant};

// Audio recording configuration constants
pub const GRACE_PERIOD: Duration = Duration::from_secs(2);
pub const SILENCE_TIMEOUT: Duration = Duration::from_millis(1500);
pub const NO_SPEECH_TIMEOUT: Duration = Duration::from_secs(5);

// Debug interval for printing adaptive levels (in sample ticks)
pub const DEBUG_PRINT_INTERVAL: usize = 400;

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

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn update_level_estimates(&mut self) {
        if !self.quiet_levels.is_empty() {
            let mut sorted_quiet: Vec<f32> = self.quiet_levels.iter().copied().collect();
            sorted_quiet.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let percentile_index = (sorted_quiet.len() as f32 * BASELINE_PERCENTILE) as usize;
            self.baseline_level = sorted_quiet
                .get(percentile_index)
                .copied()
                .unwrap_or(self.baseline_level);
        }

        if !self.active_levels.is_empty() {
            let mut sorted_active: Vec<f32> = self.active_levels.iter().copied().collect();
            sorted_active.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let percentile_index = (sorted_active.len() as f32 * ACTIVE_PERCENTILE) as usize;
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

    #[allow(clippy::cast_precision_loss)]
    pub fn add_speech_decision(&mut self, is_speech: bool) -> bool {
        if self.speech_buffer.len() >= SPEECH_BUFFER_SIZE {
            self.speech_buffer.pop_front();
        }
        self.speech_buffer.push_back(is_speech);

        let speech_count = self.speech_buffer.iter().filter(|&&x| x).count();
        let total_count = self.speech_buffer.len();
        speech_count as f32 / total_count as f32 > SPEECH_DETECTION_THRESHOLD
    }
}
