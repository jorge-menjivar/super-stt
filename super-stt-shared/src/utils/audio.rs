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
/// # Errors
///
/// Returns an error if the resampler cannot be constructed or if processing fails.
pub fn resample(
    samples: &[f32],
    from_sr: u32,
    to_sr: u32,
    quality: ResampleQuality,
) -> Result<Vec<f32>> {
    if from_sr == to_sr {
        return Ok(samples.to_vec());
    }

    let params = match quality {
        ResampleQuality::Fast => SincInterpolationParameters {
            sinc_len: 64,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Nearest,
            oversampling_factor: 16,
            window: WindowFunction::Hann,
        },
        ResampleQuality::Balanced => SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window: WindowFunction::Blackman,
        },
        ResampleQuality::HighQuality => SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Cubic,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        },
    };

    let mut resampler = Async::<f32>::new_sinc(
        f64::from(to_sr) / f64::from(from_sr),
        2.0, // max relative ratio change
        &params,
        samples.len(),
        1, // channels
        FixedAsync::Input,
    )?;

    // Mono audio: an interleaved single-channel buffer is just the flat slice.
    let input = InterleavedSlice::new(samples, 1, samples.len())?;
    let out_frames = resampler.output_frames_next();
    let mut waves_out = vec![0.0f32; out_frames];
    let mut output = InterleavedSlice::new_mut(&mut waves_out, 1, out_frames)?;
    let (_, written) = resampler.process_into_buffer(&input, &mut output, None)?;
    waves_out.truncate(written);

    Ok(waves_out)
}
