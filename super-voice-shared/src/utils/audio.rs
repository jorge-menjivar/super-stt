// SPDX-License-Identifier: GPL-3.0-only
use anyhow::Result;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

/// Apply pre-emphasis filter to boost high frequencies
/// This is commonly used in speech processing to balance the spectrum
pub fn apply_pre_emphasis(audio: &mut [f32]) {
    const PRE_EMPHASIS_COEFFICIENT: f32 = 0.97;

    if audio.len() < 2 {
        return;
    }

    // Apply the filter: y[n] = x[n] - α * x[n-1]
    for i in (1..audio.len()).rev() {
        audio[i] -= PRE_EMPHASIS_COEFFICIENT * audio[i - 1];
    }
}

/// Validate audio data for processing
///
/// # Errors
///
/// Returns an error if the audio data is invalid.
pub fn validate_audio(audio_data: &[f32], sample_rate: u32) -> Result<()> {
    use crate::validation::limits;

    if audio_data.is_empty() {
        return Err(anyhow::anyhow!("Audio data is empty"));
    }

    if sample_rate == 0 {
        return Err(anyhow::anyhow!("Invalid sample rate: 0"));
    }

    if sample_rate > limits::MAX_SAMPLE_RATE {
        return Err(anyhow::anyhow!("Sample rate too high: {sample_rate}Hz"));
    }

    // Check for invalid values
    let invalid_samples = audio_data.iter().filter(|&&x| !x.is_finite()).count();

    if invalid_samples > 0 {
        return Err(anyhow::anyhow!(
            "Audio contains {invalid_samples} invalid samples (NaN/Inf)"
        ));
    }

    // Duration cap shares the single source of truth with the protocol-layer
    // sample-count guard (`validation::limits`), so both layers agree.
    let len = u32::try_from(audio_data.len()).unwrap_or(u32::MAX);
    let duration_seconds = f64::from(len) / f64::from(sample_rate);
    let max_secs = f64::from(limits::MAX_AUDIO_DURATION_SECS);
    if duration_seconds > max_secs {
        return Err(anyhow::anyhow!(
            "Audio too long: {duration_seconds:.1}s (max {max_secs:.0}s)"
        ));
    }

    Ok(())
}

/// Normalize audio to prevent clipping and ensure consistent levels
pub fn normalize_audio(audio: &mut [f32]) {
    // Clip to [-1, 1] range
    for sample in audio.iter_mut() {
        *sample = sample.clamp(-1.0, 1.0);
    }

    // Find the maximum absolute value
    let max_val = audio.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);

    if max_val > 0.0 {
        // Normalize to 90% of max range to prevent clipping
        let scale = 0.9 / max_val;
        if scale < 1.0 {
            for sample in audio.iter_mut() {
                *sample *= scale;
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ResampleQuality {
    Fast,        // For real-time STT
    Balanced,    // For good quality/speed tradeoff
    HighQuality, // For maximum quality
}

/// Resampling with configurable quality
///
/// The output has `ceil(len * to_sr / from_sr)` samples and lines up with the
/// input in time.
///
/// # Errors
///
/// Returns an error if the resampler cannot be constructed or if processing fails.
pub fn resample(
    samples: &[f32],
    from_sr: u32,
    to_sr: u32,
    quality: ResampleQuality,
) -> Result<Vec<f32>> {
    // `process_all` feeds the clip through in chunks of this many frames, then
    // pumps silence to flush the resampler's delay. A chunk sized to the whole
    // clip would make that flush a second pass over the clip.
    const CHUNK_FRAMES: usize = 1024;

    if from_sr == to_sr {
        return Ok(samples.to_vec());
    }

    let params = match quality {
        ResampleQuality::Fast => SincInterpolationParameters {
            sinc_len: 64,
            f_cutoff: Some(0.95),
            interpolation: SincInterpolationType::Nearest,
            oversampling_factor: 16,
            window: WindowFunction::Hann,
        },
        ResampleQuality::Balanced => SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: Some(0.95),
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window: WindowFunction::Blackman,
        },
        ResampleQuality::HighQuality => SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: Some(0.95),
            interpolation: SincInterpolationType::Cubic,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        },
    };

    let mut resampler = Async::<f32>::new_sinc(
        f64::from(to_sr) / f64::from(from_sr),
        2.0, // max relative ratio change
        &params,
        CHUNK_FRAMES,
        1, // channels
        FixedAsync::Input,
    )?;

    // Mono audio: an interleaved single-channel buffer is just the flat slice.
    let input = InterleavedSlice::new(samples, 1, samples.len())?;
    // Trims the delay, so the output lines up with the input and keeps its
    // last frames. A clip resampled in pieces then joins up without gaps.
    let output = resampler.process_all(&input, samples.len(), None)?;

    Ok(output.take_data())
}

#[cfg(test)]
mod tests {
    use super::{ResampleQuality, resample};

    /// A 220 Hz tone at half amplitude, `len` samples long at `rate`.
    fn tone(rate: u32, len: u32) -> Vec<f32> {
        (0..len)
            .map(|i| {
                let t = f64::from(i) / f64::from(rate);
                #[allow(clippy::cast_possible_truncation)]
                let sample = (0.5 * (std::f64::consts::TAU * 220.0 * t).sin()) as f32;
                sample
            })
            .collect()
    }

    #[test]
    fn output_length_follows_the_rate_ratio() {
        for (from, len, want) in [
            (48_000, 4_800, 1_600),
            (44_100, 4_410, 1_600),
            (22_050, 2_205, 1_600),
            (8_000, 800, 1_600),
            // Shorter than one resampler chunk.
            (48_000, 100, 34),
        ] {
            let out = resample(&tone(from, len), from, 16_000, ResampleQuality::Fast).unwrap();
            assert_eq!(out.len(), want, "{len} samples at {from} Hz");
        }
    }

    #[test]
    fn output_lines_up_with_the_input() {
        // The resampler delays its output by half its filter length. Untrimmed,
        // that shifts this tone by about 10 samples, an error of about 0.45.
        for from in [48_000, 44_100] {
            let out = resample(&tone(from, from), from, 16_000, ResampleQuality::Fast).unwrap();
            let exact = tone(16_000, 16_000);
            // The filter sees silence past each end of the clip, so skip those.
            let worst = out[64..out.len() - 64]
                .iter()
                .zip(&exact[64..])
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(worst < 0.05, "{from} Hz: off by up to {worst}");
        }
    }

    #[test]
    fn resampling_a_stream_in_polls_keeps_every_sample() {
        // The realtime path resamples each ~100 ms poll on its own.
        let clip = tone(48_000, 48_000);
        let total: usize = clip
            .chunks(4_800)
            .map(|poll| {
                resample(poll, 48_000, 16_000, ResampleQuality::Fast)
                    .unwrap()
                    .len()
            })
            .sum();
        assert_eq!(total, 16_000);
    }

    #[test]
    fn empty_input_gives_empty_output() {
        let out = resample(&[], 48_000, 16_000, ResampleQuality::Fast).unwrap();
        assert!(out.is_empty());
    }
}
