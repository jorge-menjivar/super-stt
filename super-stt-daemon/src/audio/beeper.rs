// SPDX-License-Identifier: GPL-3.0-only

use anyhow::Result;
use cpal::Device;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const WARMUP_TONE_DURATION_MS: u64 = 20;
pub const WARMUP_TONE_FREQUENCY: f32 = 44000.0;
pub const WARMUP_DELAY_AFTER_TONE_MS: u64 = 50;

/// Timing parameters derived from the sample rate and ms inputs.
struct BeepParams {
    sample_rate: f32,
    channels: usize,
    total_samples: usize,
    fade_in_samples: usize,
    fade_out_samples: usize,
    samples_per_beep: usize,
    total_samples_with_padding: usize,
}

impl BeepParams {
    fn compute(
        device: &Device,
        config: &cpal::SupportedStreamConfig,
        frequencies: &[f32],
        duration_ms: u64,
        fade_in_ms: u64,
        fade_out_ms: u64,
    ) -> Self {
        // sample_rate() returns u32 (cpal::SampleRate = u32); routed through num_cast helper
        // since sample rates ≤ 384_000 round-trip exactly in f32.
        let sample_rate = crate::num_cast::u32_to_f32(config.sample_rate());
        let channels = usize::from(config.channels());
        // duration_ms/fade_*_ms are tiny (< a few thousand ms); their u64 values fit in u32 and
        // thus in f32 exactly. Route through num_cast helper.
        let duration_ms_f32 =
            crate::num_cast::u32_to_f32(u32::try_from(duration_ms).unwrap_or(u32::MAX));
        let fade_in_ms_f32 =
            crate::num_cast::u32_to_f32(u32::try_from(fade_in_ms).unwrap_or(u32::MAX));
        let fade_out_ms_f32 =
            crate::num_cast::u32_to_f32(u32::try_from(fade_out_ms).unwrap_or(u32::MAX));
        // f32 → usize: values are tiny sample counts (< a few million); truncation is intentional
        // and the result fits usize.
        let samples_per_beep =
            crate::num_cast::f32_to_usize(sample_rate * duration_ms_f32 / 1000.0);
        let total_samples = frequencies.len() * samples_per_beep;
        let fade_in_samples = crate::num_cast::f32_to_usize(sample_rate * fade_in_ms_f32 / 1000.0);
        let fade_out_samples =
            crate::num_cast::f32_to_usize(sample_rate * fade_out_ms_f32 / 1000.0);
        // 50 ms silence padding to let the fade-out complete before the stream stops.
        let silence_padding_samples = crate::num_cast::f32_to_usize(sample_rate * 0.05);
        let total_samples_with_padding = total_samples + silence_padding_samples;
        let _ = device; // consumed only by build_stream; held here for coherence
        Self {
            sample_rate,
            channels,
            total_samples,
            fade_in_samples,
            fade_out_samples,
            samples_per_beep,
            total_samples_with_padding,
        }
    }
}

/// Advance the sine-wave generator by one sample and return the normalised
/// float value in `[-1, 1]` (scaled by `0.3 * volume`).
///
/// Mutates `phase` and `sample_clock` in place.
fn next_sample_f32(
    sample_clock: &mut usize,
    phase: &mut f32,
    params: &BeepParams,
    frequencies: &[f32],
    volume: f32,
    finished: &AtomicBool,
) -> Option<f32> {
    if *sample_clock >= params.total_samples_with_padding {
        finished.store(true, Ordering::Relaxed);
        return None; // silence
    }
    if *sample_clock >= params.total_samples {
        *sample_clock += 1;
        return None; // padding silence
    }
    let beep_index = *sample_clock / params.samples_per_beep;
    let sample_in_beep = *sample_clock % params.samples_per_beep;
    if beep_index >= frequencies.len() {
        *sample_clock += 1;
        return None; // guard: in padding zone
    }
    let frequency = frequencies[beep_index];

    // sample_in_beep and fade_in_samples are tiny sample counts (≤ a few hundred thousand);
    // they fit in u32 and thus in f32 with no precision loss for realistic values.
    let fade_in = if beep_index == 0 && sample_in_beep < params.fade_in_samples {
        let num = crate::num_cast::u32_to_f32(u32::try_from(sample_in_beep).unwrap_or(u32::MAX));
        let den =
            crate::num_cast::u32_to_f32(u32::try_from(params.fade_in_samples).unwrap_or(u32::MAX));
        num / den
    } else {
        1.0
    };

    let samples_from_end = params.total_samples.saturating_sub(*sample_clock);
    let fade_out = if samples_from_end <= params.fade_out_samples {
        if samples_from_end == 0 {
            0.0
        } else {
            // samples_from_end - 1 and fade_out_samples - 1 are tiny sample counts
            let num = crate::num_cast::u32_to_f32(
                u32::try_from(samples_from_end - 1).unwrap_or(u32::MAX),
            );
            let den = crate::num_cast::u32_to_f32(
                u32::try_from(params.fade_out_samples - 1).unwrap_or(u32::MAX),
            );
            num / den
        }
    } else {
        1.0
    };

    let value = phase.sin() * 0.3 * volume * fade_in * fade_out;

    *phase += frequency * 2.0 * std::f32::consts::PI / params.sample_rate;
    while *phase > 2.0 * std::f32::consts::PI {
        *phase -= 2.0 * std::f32::consts::PI;
    }
    *sample_clock += 1;
    Some(value)
}

/// Build an output stream for the given device + config, wiring up the
/// per-sample generator for both F32 and I16 sample formats.
///
/// # Errors
/// Returns an error when the sample format is unsupported or `build_output_stream` fails.
fn build_stream(
    device: &Device,
    config: &cpal::SupportedStreamConfig,
    params: BeepParams,
    frequencies: Vec<f32>,
    volume: f32,
    finished: Arc<AtomicBool>,
) -> Result<cpal::Stream> {
    let channels = params.channels;
    match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let mut sample_clock = 0usize;
            let mut phase = 0.0f32;
            let stream = device
                .build_output_stream(
                    &config.config(),
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        for frame in data.chunks_mut(channels) {
                            let v = next_sample_f32(
                                &mut sample_clock,
                                &mut phase,
                                &params,
                                &frequencies,
                                volume,
                                &finished,
                            )
                            .unwrap_or(0.0);
                            for s in frame {
                                *s = v;
                            }
                        }
                    },
                    |err| log::warn!("Audio stream error: {err}"),
                    None,
                )
                .map_err(|e| anyhow::anyhow!("Failed to build F32 stream: {e}"))?;
            Ok(stream)
        }
        cpal::SampleFormat::I16 => {
            let mut sample_clock = 0usize;
            let mut phase = 0.0f32;
            let stream = device
                .build_output_stream(
                    &config.config(),
                    move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        for frame in data.chunks_mut(channels) {
                            let v = next_sample_f32(
                                &mut sample_clock,
                                &mut phase,
                                &params,
                                &frequencies,
                                volume,
                                &finished,
                            )
                            .unwrap_or(0.0);
                            // v is in [-1, 1]; scale to i16 range. The clamp guarantees the
                            // value fits in i16 before the cast — routed through num_cast helper.
                            let scaled = v * f32::from(i16::MAX);
                            let scaled_clamped =
                                scaled.clamp(f32::from(i16::MIN), f32::from(i16::MAX));
                            let iv = crate::num_cast::f32_to_i16(scaled_clamped);
                            for s in frame {
                                *s = iv;
                            }
                        }
                    },
                    |err| log::warn!("Audio stream error: {err}"),
                    None,
                )
                .map_err(|e| anyhow::anyhow!("Failed to build I16 stream: {e}"))?;
            Ok(stream)
        }
        _ => Err(anyhow::anyhow!(
            "Unsupported sample format for beep playback"
        )),
    }
}

/// Spin-wait until the stream finishes or the timeout elapses, then sleep
/// an extra buffer-flush period and drop the stream.
fn wait_for_completion(
    stream: cpal::Stream,
    finished: &AtomicBool,
    total_duration_ms: u64,
    freq_count: usize,
    sample_rate: f32,
) {
    let beep_start = std::time::Instant::now();
    // freq_count is a tiny count of frequencies (typically 1–4); fits u64.
    let freq_count_u64 = u64::try_from(freq_count).unwrap_or(u64::MAX);
    let beep_timeout = Duration::from_millis(total_duration_ms * freq_count_u64 + 2000);
    while !finished.load(Ordering::Relaxed) {
        if beep_start.elapsed() > beep_timeout {
            log::warn!("Beep playback timed out after {:?}", beep_start.elapsed());
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // Most audio drivers use 256–2048 sample buffers; assume ~1024 worst-case.
    // 1024/sample_rate*1000 is in the range [~10, ~46] ms for typical rates, then clamped to
    // [100, 200]. The f32 result is non-negative and well within u64 range; clamp before cast.
    let buffer_flush_ms_f32 = (1024.0_f32 / sample_rate * 1000.0).clamp(0.0, f32::MAX);
    let buffer_flush_ms = crate::num_cast::f32_to_u64(buffer_flush_ms_f32);
    let buffer_flush_time = Duration::from_millis(buffer_flush_ms.clamp(100, 200));
    std::thread::sleep(buffer_flush_time);

    drop(stream);
}

/// Play a sequence of beeps on a freshly initialized output device.
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

    let params = BeepParams::compute(
        &device,
        &config,
        frequencies,
        duration_ms,
        fade_in_ms,
        fade_out_ms,
    );
    let sample_rate = params.sample_rate;

    let finished = Arc::new(AtomicBool::new(false));
    let stream = build_stream(
        &device,
        &config,
        params,
        frequencies.to_vec(),
        volume,
        Arc::clone(&finished),
    )?;

    stream
        .play()
        .map_err(|e| anyhow::anyhow!("Failed to play beep: {e}"))?;

    wait_for_completion(
        stream,
        &finished,
        duration_ms,
        frequencies.len(),
        sample_rate,
    );
    Ok(())
}

/// Async wrapper over [`play_beep_sequence`]. The underlying call sets up a cpal
/// stream and then spin-waits (with `std::thread::sleep`) for the whole beep to
/// finish, which would stall an async worker for the sound's full duration; run
/// it on a blocking thread instead (Tier 3 #4).
///
/// # Errors
///
/// Propagates the playback error (no output device, or stream build/play
/// failure), or reports a task-join failure.
pub async fn play_beep_sequence_async(
    frequencies: Vec<f32>,
    duration_ms: u64,
    fade_in_ms: u64,
    fade_out_ms: u64,
    volume: f32,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        play_beep_sequence(&frequencies, duration_ms, fade_in_ms, fade_out_ms, volume)
    })
    .await
    .map_err(|e| anyhow::anyhow!("beep playback task failed: {e}"))?
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
                let num = crate::num_cast::u32_to_f32(
                    u32::try_from(samples_from_total_end - 1).unwrap_or(u32::MAX),
                );
                let den = crate::num_cast::u32_to_f32(
                    u32::try_from(fade_out_samples - 1).unwrap_or(u32::MAX),
                );
                num / den
            }
        } else {
            1.0
        }
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
        let sample_rates: Vec<f32> = vec![44100.0, 48000.0, 96000.0, 22050.0];

        for sample_rate in sample_rates {
            // Mirror production code in wait_for_completion
            let flush_ms_f32 = (1024.0_f32 / sample_rate * 1000.0).clamp(0.0, f32::MAX);
            let buffer_flush_ms = crate::num_cast::f32_to_u64(flush_ms_f32);
            let buffer_flush_time = buffer_flush_ms.clamp(100, 200);

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
        let result = play_beep_sequence(&[], 100, 10, 10, 1.0);
        assert!(
            result.is_ok(),
            "Empty frequency array should not cause error"
        );
    }
}
