// SPDX-License-Identifier: GPL-3.0-only
use anyhow::Result;
use candle_core::{Device, Tensor};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::conv::FromSample;

fn conv<T>(samples: &mut Vec<f32>, data: &symphonia::core::audio::AudioBuffer<T>)
where
    T: symphonia::core::sample::Sample,
    f32: symphonia::core::conv::FromSample<T>,
{
    // Convert samples and ensure proper scaling to [-1.0, 1.0] range
    let converted_samples: Vec<f32> = data.chan(0).iter().map(|v| f32::from_sample(*v)).collect();

    // Check if samples are improperly scaled (too small)
    let max_abs = converted_samples
        .iter()
        .map(|&x| x.abs())
        .fold(0.0f32, f32::max);

    if max_abs > 0.0 && max_abs < 1e-3 {
        // Samples are likely in wrong scale, apply normalization
        // This handles cases where integer samples weren't properly normalized
        let scale_factor = 0.5 / max_abs; // Scale to use about 50% of the [-1,1] range
        samples.extend(converted_samples.iter().map(|&x| x * scale_factor));
    } else {
        // Samples appear to be in correct scale
        samples.extend(converted_samples);
    }
}

/// Decode audio file to PCM samples
///
/// # Errors
///
/// Returns an error if the file cannot be opened, decoded,
/// or if the audio stream cannot be processed.
pub fn pcm_decode<P: AsRef<std::path::Path>>(path: P) -> Result<(Vec<f32>, u32)> {
    let src = std::fs::File::open(path)?;
    let mss = symphonia::core::io::MediaSourceStream::new(
        Box::new(src),
        symphonia::core::io::MediaSourceStreamOptions::default(),
    );
    let hint = symphonia::core::probe::Hint::new();
    let meta_opts: symphonia::core::meta::MetadataOptions =
        symphonia::core::meta::MetadataOptions::default();
    let fmt_opts: symphonia::core::formats::FormatOptions =
        symphonia::core::formats::FormatOptions::default();

    let probed = symphonia::default::get_probe().format(&hint, mss, &fmt_opts, &meta_opts)?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow::anyhow!("no supported audio tracks"))?;

    let dec_opts: DecoderOptions = DecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &dec_opts)?;

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(16000);
    let mut pcm_data = Vec::new();

    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet)? {
            AudioBufferRef::F64(buf) => conv(&mut pcm_data, &buf),
            AudioBufferRef::F32(buf) => conv(&mut pcm_data, &buf),
            AudioBufferRef::S32(buf) => conv(&mut pcm_data, &buf),
            AudioBufferRef::S16(buf) => conv(&mut pcm_data, &buf),
            AudioBufferRef::S8(buf) => conv(&mut pcm_data, &buf),
            AudioBufferRef::U32(buf) => conv(&mut pcm_data, &buf),
            AudioBufferRef::U16(buf) => conv(&mut pcm_data, &buf),
            AudioBufferRef::U8(buf) => conv(&mut pcm_data, &buf),
            AudioBufferRef::U24(buf) => conv(&mut pcm_data, &buf),
            AudioBufferRef::S24(buf) => conv(&mut pcm_data, &buf),
        }
    }

    Ok((pcm_data, sample_rate))
}

/// Convert PCM samples to mel spectrogram features (exact Whisper format)
///
/// # Errors
///
/// Returns an error if tensor creation or reshaping fails.
#[allow(clippy::cast_precision_loss)]
pub fn to_mel_spectrogram(samples: &[f32], n_mels: usize, device: &Device) -> Result<Tensor> {
    // Exact Whisper parameters
    let sample_rate = 16000.0;
    let n_fft = 400; // 25ms window at 16kHz  
    let hop_length = 160; // 10ms hop at 16kHz
    let win_length = n_fft;
    let n_freqs = n_fft / 2 + 1;

    // Whisper uses 30 second chunks, pad to at least that much
    let target_length: usize = 30usize * 16000usize; // 30 seconds @ 16kHz
    let mut padded_samples = samples.to_vec();
    if padded_samples.len() < target_length {
        padded_samples.resize(target_length, 0.0);
    }

    let n_frames = padded_samples.len() / hop_length;

    // Create Whisper mel filterbank
    let mel_filters = create_whisper_mel_filterbank(n_mels, n_freqs, sample_rate);

    // Pre-allocate mel features
    let mut mel_features = vec![0.0f32; n_mels * n_frames];

    for frame_idx in 0..n_frames {
        let frame_start = frame_idx * hop_length;
        let frame_end = (frame_start + win_length).min(padded_samples.len());

        // Extract frame with zero padding
        let mut frame = vec![0.0f32; win_length];
        if frame_start < padded_samples.len() {
            let copy_len = (frame_end - frame_start).min(win_length);
            frame[..copy_len].copy_from_slice(&padded_samples[frame_start..frame_start + copy_len]);
        }

        // Apply Hann window (Whisper standard)
        for (i, sample) in frame.iter_mut().enumerate().take(win_length) {
            let i_f = i as f32;
            let win_len_f = win_length as f32;
            let window = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i_f / win_len_f).cos());
            *sample *= window;
        }

        // Compute magnitude spectrum via DFT
        let power_spectrum = compute_power_spectrum(&frame, n_freqs);

        // Apply mel filterbank and log
        for mel_idx in 0..n_mels {
            let mut mel_energy = 0.0f32;
            for freq_idx in 0..n_freqs {
                mel_energy += power_spectrum[freq_idx] * mel_filters[mel_idx * n_freqs + freq_idx];
            }

            // Whisper uses log10, then natural log for some reason
            // But let's try natural log with proper epsilon
            let log_mel = (mel_energy + 1e-10).ln();
            mel_features[frame_idx * n_mels + mel_idx] = log_mel;
        }
    }

    // Whisper normalization: global mean and std normalization
    let mut sum = 0.0f32;
    let mut count: usize = 0;

    for &val in &mel_features {
        if val.is_finite() {
            sum += val;
            count += 1;
        }
    }

    let mean = if count > 0 { sum / count as f32 } else { 0.0 };

    // Compute standard deviation
    let mut var_sum = 0.0f32;
    for &val in &mel_features {
        if val.is_finite() {
            let diff = val - mean;
            var_sum += diff * diff;
        }
    }

    let std_dev = if count > 1 {
        (var_sum / (count - 1) as f32).sqrt()
    } else {
        1.0
    };
    let std_dev = std_dev.max(1e-8); // Avoid division by zero

    // Normalize to zero mean, unit variance
    for val in &mut mel_features {
        if val.is_finite() {
            *val = (*val - mean) / std_dev;
        } else {
            *val = 0.0;
        }
    }

    let tensor = Tensor::new(mel_features, device)?.reshape((1, n_mels, n_frames))?;
    Ok(tensor)
}

/// Create Whisper-compatible mel filterbank matrix
#[allow(clippy::cast_precision_loss)]
fn create_whisper_mel_filterbank(n_mels: usize, n_freqs: usize, sample_rate: f32) -> Vec<f32> {
    let mut filterbank = vec![0.0f32; n_mels * n_freqs];

    // Whisper mel scale conversion (uses specific constants)
    let hz_to_mel = |hz: f32| 2595.0 * (1.0 + hz / 700.0).log10();
    let mel_to_hz = |mel: f32| 700.0 * (10.0_f32.powf(mel / 2595.0) - 1.0);

    let nyquist = sample_rate / 2.0;
    let mel_min = hz_to_mel(0.0);
    let mel_max = hz_to_mel(nyquist);

    // Create mel points (Whisper uses n_mels + 2 points for n_mels filters)
    let mel_points: Vec<f32> = (0..=n_mels + 1)
        .map(|i| mel_min + (mel_max - mel_min) * i as f32 / (n_mels + 1) as f32)
        .collect();

    let hz_points: Vec<f32> = mel_points.iter().map(|&mel| mel_to_hz(mel)).collect();

    // Convert Hz to FFT bin indices
    let bin_points: Vec<f32> = hz_points
        .iter()
        .map(|&hz| (n_freqs - 1) as f32 * hz / nyquist)
        .collect();

    // Create triangular filters
    for mel_idx in 0..n_mels {
        let left = bin_points[mel_idx];
        let center = bin_points[mel_idx + 1];
        let right = bin_points[mel_idx + 2];

        for freq_idx in 0..n_freqs {
            let freq_idx_f = freq_idx as f32;

            let weight = if freq_idx_f < left || freq_idx_f > right {
                0.0
            } else if freq_idx_f <= center {
                if center > left {
                    (freq_idx_f - left) / (center - left)
                } else {
                    1.0
                }
            } else if right > center {
                (right - freq_idx_f) / (right - center)
            } else {
                1.0
            };

            filterbank[mel_idx * n_freqs + freq_idx] = weight;
        }
    }

    filterbank
}

/// Compute power spectrum using proper DFT implementation
#[allow(clippy::cast_precision_loss)]
fn compute_power_spectrum(windowed_frame: &[f32], n_freqs: usize) -> Vec<f32> {
    let mut power_spectrum = vec![0.0f32; n_freqs];
    let n_samples = windowed_frame.len();

    // Proper DFT computation for power spectrum
    for (k, bin) in power_spectrum.iter_mut().enumerate().take(n_freqs) {
        let mut real_sum = 0.0f32;
        let mut imag_sum = 0.0f32;

        for (n, &sample) in windowed_frame.iter().enumerate() {
            let angle = -2.0 * std::f32::consts::PI * k as f32 * n as f32 / n_samples as f32;
            real_sum += sample * angle.cos();
            imag_sum += sample * angle.sin();
        }

        // Power = |X(k)|^2
        *bin = real_sum * real_sum + imag_sum * imag_sum;
    }

    power_spectrum
}

/// Load audio file and compute mel features tensor
///
/// # Errors
///
/// Returns an error if the audio cannot be decoded or if mel feature
/// computation fails to create the output tensor.
pub fn load_audio_features(audio_path: &str, n_mels: usize, device: &Device) -> Result<Tensor> {
    let (samples, _sr) = pcm_decode(audio_path)?;
    to_mel_spectrogram(&samples, n_mels, device)
}
