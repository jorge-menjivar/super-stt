// SPDX-License-Identifier: GPL-3.0-only
use anyhow::Result;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

#[derive(Debug, Clone, Copy)]
pub enum ResampleQuality {
    Fast,        // Lowest latency, for hot paths
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
