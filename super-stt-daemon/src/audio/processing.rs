// SPDX-License-Identifier: GPL-3.0-only

use crate::audio::state::{GRACE_PERIOD, NO_SPEECH_TIMEOUT, RecordingState, SILENCE_TIMEOUT};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use super_stt_shared::models::audio::AudioLevel;
use tokio::sync::broadcast;

/// Process mono audio samples for recording state and levels
///
/// Handles poisoned locks gracefully by logging warnings and recovering the data.
pub fn process_audio_samples(
    mono_samples: &[f32],
    buffer: &Arc<Mutex<VecDeque<f32>>>,
    state: &Arc<Mutex<RecordingState>>,
    level_tx: &broadcast::Sender<AudioLevel>,
) {
    let mut buffer = match buffer.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            log::warn!("Audio buffer lock was poisoned in processing, attempting recovery");
            poisoned.into_inner()
        }
    };
    let mut state = match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            log::warn!("Recording state lock was poisoned in processing, attempting recovery");
            poisoned.into_inner()
        }
    };

    #[allow(clippy::cast_precision_loss)]
    let rms: f32 =
        (mono_samples.iter().map(|&x| x * x).sum::<f32>() / mono_samples.len() as f32).sqrt();

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

#[allow(clippy::cast_precision_loss)]
pub fn process_audio_data_f32_with_streaming(
    data: &[f32],
    channels: usize,
    buffer: &Arc<Mutex<VecDeque<f32>>>,
    state: &Arc<Mutex<RecordingState>>,
    level_tx: &broadcast::Sender<AudioLevel>,
    samples_tx: &tokio::sync::mpsc::UnboundedSender<Vec<f32>>,
) {
    let mono_samples: Vec<f32> = data
        .chunks(channels)
        .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
        .collect();
    let _ = samples_tx.send(mono_samples.clone());
    process_audio_samples(&mono_samples, buffer, state, level_tx);
}

#[allow(clippy::cast_precision_loss)]
pub fn process_audio_data_i16_with_streaming(
    data: &[i16],
    channels: usize,
    buffer: &Arc<Mutex<VecDeque<f32>>>,
    state: &Arc<Mutex<RecordingState>>,
    level_tx: &broadcast::Sender<AudioLevel>,
    samples_tx: &tokio::sync::mpsc::UnboundedSender<Vec<f32>>,
) {
    let samples: Vec<f32> = data.iter().map(|&s| f32::from(s) / 32768.0).collect();
    let mono_samples: Vec<f32> = samples
        .chunks(channels)
        .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
        .collect();
    let _ = samples_tx.send(mono_samples.clone());
    process_audio_samples(&mono_samples, buffer, state, level_tx);
}
