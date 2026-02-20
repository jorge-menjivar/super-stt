// SPDX-License-Identifier: GPL-3.0-only
use anyhow::Result;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
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
#[allow(clippy::cast_precision_loss)]
pub fn validate_audio(audio_data: &[f32], sample_rate: u32) -> Result<()> {
    if audio_data.is_empty() {
        return Err(anyhow::anyhow!("Audio data is empty"));
    }

    if sample_rate == 0 {
        return Err(anyhow::anyhow!("Invalid sample rate: 0"));
    }

    if sample_rate > 96000 {
        return Err(anyhow::anyhow!("Sample rate too high: {sample_rate}Hz"));
    }

    // Check for invalid values
    let invalid_samples = audio_data.iter().filter(|&&x| !x.is_finite()).count();

    if invalid_samples > 0 {
        return Err(anyhow::anyhow!(
            "Audio contains {invalid_samples} invalid samples (NaN/Inf)"
        ));
    }

    let duration_seconds = audio_data.len() as f64 / f64::from(sample_rate);
    if duration_seconds > 300.0 {
        return Err(anyhow::anyhow!(
            "Audio too long: {duration_seconds:.1}s (max 300s)"
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
///
/// # Panics
///
/// Panics if the resampler returns no output frames (unexpected).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
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

    let mut resampler = SincFixedIn::<f32>::new(
        f64::from(to_sr) / f64::from(from_sr),
        2.0, // max relative ratio change
        params,
        samples.len(),
        1, // channels
    )?;

    let waves_in = vec![samples.to_vec()];
    let waves_out = resampler.process(&waves_in, None)?;

    Ok(waves_out.into_iter().next().unwrap())
}
