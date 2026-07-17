// SPDX-License-Identifier: GPL-3.0-only

use crate::audio::state::{GRACE_PERIOD, NO_SPEECH_TIMEOUT, RecordingState, SILENCE_TIMEOUT};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;
use super_stt_shared::models::audio_level::AudioLevel;
use tokio::sync::broadcast;

/// Process mono audio samples for recording state and levels.
///
/// Uses `parking_lot::Mutex`, whose guards carry no poison state — a panic
/// while another thread holds the lock cannot poison it, so there is no
/// recovery to hand-roll. cpal invokes this on the real-time input thread, and
/// `parking_lot` locks are safe in sync callbacks.
pub fn process_audio_samples(
    mono_samples: &[f32],
    buffer: &Arc<Mutex<VecDeque<f32>>>,
    state: &Arc<Mutex<RecordingState>>,
    level_tx: &broadcast::Sender<AudioLevel>,
) {
    let mut buffer = buffer.lock();
    let mut state = state.lock();

    // mono_samples.len() is a chunk size (typically a few hundred to a few thousand);
    // fits in u32 and thus in f32 exactly.
    let len_f32 =
        crate::num_cast::u32_to_f32(u32::try_from(mono_samples.len()).unwrap_or(u32::MAX));
    let rms: f32 = (mono_samples.iter().map(|&x| x * x).sum::<f32>() / len_f32).sqrt();

    buffer.extend(mono_samples);

    if state.recording_start.is_none() {
        state.recording_start = Some(Instant::now());
    }

    let current_threshold = state.get_speech_threshold();
    let raw_speech_decision = rms > current_threshold;

    let recent_activity = state.speech_buffer.iter().rev().take(3).any(|&x| x);
    state.update_adaptive_levels(rms, recent_activity);

    let is_speech = state.add_speech_decision(raw_speech_decision);

    if is_speech {
        if !state.recording {
            state.recording = true;
        }
        state.silence_start = None;
    }

    let in_grace_period = if let Some(recording_start) = state.recording_start {
        recording_start.elapsed() < GRACE_PERIOD
    } else {
        true
    };

    if !in_grace_period && !state.silence_detection_disabled {
        if state.recording {
            if !is_speech {
                if state.silence_start.is_none() {
                    state.silence_start = Some(Instant::now());
                }
                if let Some(silence_start) = state.silence_start
                    && silence_start.elapsed() >= SILENCE_TIMEOUT
                    && !state.stop_requested
                {
                    state.stop_requested = true;
                }
            }
        } else if let Some(recording_start) = state.recording_start
            && recording_start.elapsed() >= NO_SPEECH_TIMEOUT
            && !state.stop_requested
        {
            log::warn!(
                "⚠️  No speech detected for {} seconds, stopping...",
                NO_SPEECH_TIMEOUT.as_secs()
            );
            state.stop_requested = true;
        }
    }

    let audio_level = AudioLevel {
        level: rms,
        is_speech,
        timestamp: Instant::now(),
    };
    let _ = level_tx.send(audio_level);
}

pub fn process_audio_data_f32_with_streaming(
    data: &[f32],
    channels: usize,
    buffer: &Arc<Mutex<VecDeque<f32>>>,
    state: &Arc<Mutex<RecordingState>>,
    level_tx: &broadcast::Sender<AudioLevel>,
    samples_tx: &tokio::sync::mpsc::UnboundedSender<Vec<f32>>,
) {
    // channels is typically 1–8; fits u16 and thus u32 and f32 exactly.
    let channels_f32 = f32::from(u16::try_from(channels).unwrap_or(u16::MAX));
    let mono_samples: Vec<f32> = data
        .chunks(channels)
        .map(|chunk| chunk.iter().sum::<f32>() / channels_f32)
        .collect();
    // Process against the borrowed buffer first, then MOVE the mono buffer into
    // the analysis channel — one allocation instead of an extra clone on the
    // audio RT thread (audit 2 Tier 3 #5).
    process_audio_samples(&mono_samples, buffer, state, level_tx);
    let _ = samples_tx.send(mono_samples);
}

pub fn process_audio_data_i16_with_streaming(
    data: &[i16],
    channels: usize,
    buffer: &Arc<Mutex<VecDeque<f32>>>,
    state: &Arc<Mutex<RecordingState>>,
    level_tx: &broadcast::Sender<AudioLevel>,
    samples_tx: &tokio::sync::mpsc::UnboundedSender<Vec<f32>>,
) {
    // channels is typically 1–8; fits u16 and thus u32 and f32 exactly.
    let channels_f32 = f32::from(u16::try_from(channels).unwrap_or(u16::MAX));
    // Downmix straight from the `&[i16]` slice into a single mono `Vec<f32>` —
    // no throwaway interleaved-f32 Vec and no extra clone (three allocations →
    // one) on the audio RT thread (audit 2 Tier 3 #5).
    let mono_samples: Vec<f32> = data
        .chunks(channels)
        .map(|chunk| chunk.iter().map(|&s| f32::from(s) / 32768.0).sum::<f32>() / channels_f32)
        .collect();
    process_audio_samples(&mono_samples, buffer, state, level_tx);
    let _ = samples_tx.send(mono_samples);
}
