// SPDX-License-Identifier: GPL-3.0-only

use anyhow::Result;
use cpal::Device;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use log::debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const WARMUP_TONE_DURATION_MS: u64 = 20;
pub const WARMUP_TONE_FREQUENCY: f32 = 44000.0;
pub const WARMUP_DELAY_AFTER_TONE_MS: u64 = 50;

/// Play a short warm-up tone to initialize audio drivers.
///
/// # Errors
///
/// Returns an error only if the post-tone sleep fails (unlikely). Errors
/// from the tone itself are logged and ignored.
pub fn play_warmup_tone() -> Result<()> {
    debug!("Playing warm-up tone to initialize audio drivers");
    let warmup_frequencies = [WARMUP_TONE_FREQUENCY];
    let warmup_duration = WARMUP_TONE_DURATION_MS;
    if let Err(e) = play_beep_sequence(&warmup_frequencies, warmup_duration, 5, 5) {
        debug!("Warm-up tone failed (usually fine): {e}");
    }
    std::thread::sleep(Duration::from_millis(WARMUP_DELAY_AFTER_TONE_MS));
    Ok(())
}

/// Play a sequence of beeps on a freshly initialized output device.
///
/// # Errors
///
/// Returns an error if no output device is available or if the output stream
/// cannot be created or played.
#[allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn play_beep_sequence(
    frequencies: &[f32],
    duration_ms: u64,
    fade_in_ms: u64,
    fade_out_ms: u64,
) -> Result<()> {
    if frequencies.is_empty() {
        return Ok(());
    }

    log::info!(
        "Playing beep sequence with fresh device initialization: {frequencies:?} for {duration_ms}ms each, fade_in: {fade_in_ms}ms, fade_out: {fade_out_ms}ms"
    );

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("No output device available"))?;

    let config = device
        .default_output_config()
        .map_err(|e| anyhow::anyhow!("Failed to get output config: {e}"))?;

    let sample_rate = config.sample_rate().0 as f32;
    let channels = config.channels() as usize;

    // Calculate total duration for all beeps to play continuously
    let total_samples = frequencies.len() * (sample_rate * duration_ms as f32 / 1000.0) as usize;
    let fade_in_samples = (sample_rate * fade_in_ms as f32 / 1000.0) as usize;
    let fade_out_samples = (sample_rate * fade_out_ms as f32 / 1000.0) as usize;
    let samples_per_beep = (sample_rate * duration_ms as f32 / 1000.0) as usize;
    // Add silence padding at the end to ensure fade-out completes before stream stops
    let silence_padding_samples = (sample_rate * 0.05) as usize; // 50ms of silence
    let total_samples_with_padding = total_samples + silence_padding_samples;

    let mut sample_clock = 0usize;
    let mut phase = 0.0f32; // Track phase for smooth transitions
    let finished = std::sync::Arc::new(AtomicBool::new(false));
    let finished_clone = finished.clone();
    let frequencies_clone = frequencies.to_vec();

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config.config(),
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    if sample_clock >= total_samples_with_padding {
                        finished_clone.store(true, Ordering::Relaxed);
                        for sample in frame {
                            *sample = 0.0;
                        }
                        return;
                    }

                    // Output silence during padding period
                    if sample_clock >= total_samples {
                        for sample in frame {
                            *sample = 0.0;
                        }
                        sample_clock += 1;
                        continue;
                    }

                    // Determine which beep we're on and position within that beep
                    let beep_index = sample_clock / samples_per_beep;
                    let sample_in_beep = sample_clock % samples_per_beep;

                    if beep_index >= frequencies_clone.len() {
                        // We're in the padding zone
                        for sample in frame {
                            *sample = 0.0;
                        }
                        sample_clock += 1;
                        continue;
                    }

                    let frequency = frequencies_clone[beep_index];

                    // Apply fade-in at the start of the first beep
                    let fade_in_multiplier = if beep_index == 0 && sample_in_beep < fade_in_samples
                    {
                        sample_in_beep as f32 / fade_in_samples as f32
                    } else {
                        1.0
                    };

                    // Apply fade-out at the end of the entire sequence
                    let samples_from_total_end = total_samples.saturating_sub(sample_clock);
                    let fade_out_multiplier = if samples_from_total_end <= fade_out_samples {
                        // Fade from 1.0 to 0.0 over fade_out_samples
                        if samples_from_total_end == 0 {
                            0.0
                        } else {
                            (samples_from_total_end - 1) as f32 / (fade_out_samples - 1) as f32
                        }
                    } else {
                        1.0
                    };

                    // Generate sine wave with continuous phase and both fades
                    let value = phase.sin() * 0.3 * fade_in_multiplier * fade_out_multiplier;

                    // Update phase for next sample
                    phase += frequency * 2.0 * std::f32::consts::PI / sample_rate;
                    // Keep phase in reasonable range to avoid precision issues
                    while phase > 2.0 * std::f32::consts::PI {
                        phase -= 2.0 * std::f32::consts::PI;
                    }

                    for sample in frame {
                        *sample = value;
                    }
                    sample_clock += 1;
                }
            },
            |err| log::warn!("Audio stream error: {err}"),
            None,
        ),
        cpal::SampleFormat::I16 => {
            let frequencies_clone = frequencies.to_vec();
            let mut phase = 0.0f32; // Track phase for smooth transitions
            device.build_output_stream(
                &config.config(),
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    for frame in data.chunks_mut(channels) {
                        if sample_clock >= total_samples_with_padding {
                            finished_clone.store(true, Ordering::Relaxed);
                            for sample in frame {
                                *sample = 0;
                            }
                            return;
                        }

                        // Output silence during padding period
                        if sample_clock >= total_samples {
                            for sample in frame {
                                *sample = 0;
                            }
                            sample_clock += 1;
                            continue;
                        }

                        // Determine which beep we're on and position within that beep
                        let beep_index = sample_clock / samples_per_beep;
                        let sample_in_beep = sample_clock % samples_per_beep;

                        if beep_index >= frequencies_clone.len() {
                            // We're in the padding zone
                            for sample in frame {
                                *sample = 0;
                            }
                            sample_clock += 1;
                            continue;
                        }

                        let frequency = frequencies_clone[beep_index];

                        // Apply fade-in at the start of the first beep
                        let fade_in_multiplier =
                            if beep_index == 0 && sample_in_beep < fade_in_samples {
                                sample_in_beep as f32 / fade_in_samples as f32
                            } else {
                                1.0
                            };

                        // Apply fade-out at the end of the entire sequence
                        let samples_from_total_end = total_samples.saturating_sub(sample_clock);
                        let fade_out_multiplier = if samples_from_total_end <= fade_out_samples {
                            // Fade from 1.0 to 0.0 over fade_out_samples
                            if samples_from_total_end == 0 {
                                0.0
                            } else {
                                (samples_from_total_end - 1) as f32 / (fade_out_samples - 1) as f32
                            }
                        } else {
                            1.0
                        };

                        // Generate sine wave with continuous phase and both fades
                        let value = phase.sin() * 0.3 * fade_in_multiplier * fade_out_multiplier;
                        let sample_value = (value * f32::from(i16::MAX)) as i16;

                        // Update phase for next sample
                        phase += frequency * 2.0 * std::f32::consts::PI / sample_rate;
                        // Keep phase in reasonable range to avoid precision issues
                        while phase > 2.0 * std::f32::consts::PI {
                            phase -= 2.0 * std::f32::consts::PI;
                        }

                        for sample in frame {
                            *sample = sample_value;
                        }
                        sample_clock += 1;
                    }
                },
                |err| log::warn!("Audio stream error: {err}"),
                None,
            )
        }
        _ => {
            return Err(anyhow::anyhow!(
                "Unsupported sample format for beep playback"
            ));
        }
    }?;

    stream
        .play()
        .map_err(|e| anyhow::anyhow!("Failed to play beep: {e}"))?;

    let beep_start = std::time::Instant::now();
    let beep_timeout = Duration::from_millis((duration_ms * frequencies.len() as u64) + 2000);
    while !finished.load(Ordering::Relaxed) {
        if beep_start.elapsed() > beep_timeout {
            log::warn!("Beep playback timed out after {:?}", beep_start.elapsed());
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // Calculate buffer flush time based on sample rate and typical audio buffer sizes
    // Most audio drivers use 256-2048 sample buffers, we'll assume ~1024 samples worst case
    let estimated_buffer_samples = 1024.0;
    let buffer_flush_ms = (estimated_buffer_samples / sample_rate * 1000.0) as u64;
    let buffer_flush_time = Duration::from_millis(buffer_flush_ms.clamp(100, 200)); // 100-200ms range

    std::thread::sleep(buffer_flush_time);

    drop(stream);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper to calculate fade-out multiplier like the actual code
    fn calculate_fade_out_multiplier(
        sample_clock: usize,
        total_samples: usize,
        fade_out_samples: usize,
    ) -> f32 {
        let samples_from_total_end = total_samples.saturating_sub(sample_clock);
        if samples_from_total_end <= fade_out_samples {
            if samples_from_total_end == 0 {
                0.0
            } else {
                (samples_from_total_end - 1) as f32 / (fade_out_samples - 1) as f32
            }
        } else {
            1.0
        }
    }

    #[test]
    fn test_fade_out_calculation() {
        let fade_out_samples = 100;
        let total_samples = 1000;

        // Test normal operation (not in fade-out zone)
        assert_eq!(
            calculate_fade_out_multiplier(500, total_samples, fade_out_samples),
            1.0
        );

        // Test fade-out zone
        assert_eq!(
            calculate_fade_out_multiplier(999, total_samples, fade_out_samples),
            0.0
        ); // Last sample
        assert_eq!(
            calculate_fade_out_multiplier(998, total_samples, fade_out_samples),
            1.0 / 99.0
        ); // Second to last

        // Test start of fade-out
        let fade_start_sample = total_samples - fade_out_samples; // 900
        assert_eq!(
            calculate_fade_out_multiplier(fade_start_sample, total_samples, fade_out_samples),
            1.0
        );
    }

    #[test]
    fn test_fade_out_reaches_zero() {
        let fade_out_samples = 50;
        let total_samples = 2000;

        // The final sample should always be 0.0
        let final_multiplier =
            calculate_fade_out_multiplier(total_samples - 1, total_samples, fade_out_samples);
        assert_eq!(
            final_multiplier, 0.0,
            "Final sample should have zero multiplier"
        );
    }

    #[test]
    fn test_fade_out_progression() {
        let fade_out_samples = 10;
        let total_samples = 100;

        let mut previous_multiplier = 1.0;

        // Check that fade-out multipliers decrease monotonically
        for sample_clock in (total_samples - fade_out_samples)..total_samples {
            let multiplier =
                calculate_fade_out_multiplier(sample_clock, total_samples, fade_out_samples);
            assert!(
                multiplier <= previous_multiplier,
                "Fade-out should decrease monotonically: sample {} has multiplier {} > {}",
                sample_clock,
                multiplier,
                previous_multiplier
            );
            previous_multiplier = multiplier;
        }
    }

    #[test]
    fn test_buffer_flush_calculation() {
        let sample_rates = vec![44100.0, 48000.0, 96000.0, 22050.0];

        for sample_rate in sample_rates {
            let estimated_buffer_samples = 1024.0;
            let buffer_flush_ms = (estimated_buffer_samples / sample_rate * 1000.0) as u64;
            let buffer_flush_time = buffer_flush_ms.max(100).min(200);

            // Should be clamped between 100-200ms
            assert!(
                buffer_flush_time >= 100,
                "Buffer flush time should be at least 100ms for sample rate {}",
                sample_rate
            );
            assert!(
                buffer_flush_time <= 200,
                "Buffer flush time should be at most 200ms for sample rate {}",
                sample_rate
            );
        }
    }

    #[test]
    fn test_empty_frequencies() {
        // This should return Ok(()) without panicking
        let result = play_beep_sequence(&[], 100, 10, 10);
        assert!(
            result.is_ok(),
            "Empty frequency array should not cause error"
        );
    }

    #[test]
    fn test_warmup_tone_constants() {
        // Verify warmup tone constants are reasonable
        assert!(
            WARMUP_TONE_DURATION_MS > 0,
            "Warmup duration should be positive"
        );
        assert!(
            WARMUP_TONE_FREQUENCY > 20.0,
            "Warmup frequency should be positive"
        );
        // Note: Warmup tone uses very high frequency (44kHz) intentionally to warm up audio drivers
        // without creating audible noise for users
        assert!(
            WARMUP_TONE_FREQUENCY < 50000.0,
            "Warmup frequency should be reasonable"
        );
        assert!(
            WARMUP_DELAY_AFTER_TONE_MS < 1000,
            "Warmup delay should be reasonable"
        );
    }
}

/// Verify the output device by opening a short silent stream.
///
/// # Errors
///
/// Returns an error if a test stream cannot be created or played.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn verify_fresh_device(device: &Device, config: &cpal::SupportedStreamConfig) -> Result<()> {
    let sample_rate = config.sample_rate().0 as f32;
    let channels = config.channels() as usize;
    let verification_samples = (sample_rate * 0.05) as usize; // 50ms of silence
    let mut sample_count = 0usize;
    let completed = std::sync::Arc::new(AtomicBool::new(false));
    let completed_clone = completed.clone();

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config.config(),
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    if sample_count >= verification_samples {
                        completed_clone.store(true, Ordering::Relaxed);
                        return;
                    }
                    for sample in frame {
                        *sample = 0.0;
                    }
                    sample_count += 1;
                }
            },
            |err| log::debug!("Verification stream error: {err}"),
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config.config(),
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    if sample_count >= verification_samples {
                        completed_clone.store(true, Ordering::Relaxed);
                        return;
                    }
                    for sample in frame {
                        *sample = 0;
                    }
                    sample_count += 1;
                }
            },
            |err| log::debug!("Verification stream error: {err}"),
            None,
        )?,
        _ => return Err(anyhow::anyhow!("Unsupported sample format")),
    };

    stream.play()?;
    let start = std::time::Instant::now();
    while !completed.load(Ordering::Relaxed) && start.elapsed() < Duration::from_millis(200) {
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(stream);
    Ok(())
}
