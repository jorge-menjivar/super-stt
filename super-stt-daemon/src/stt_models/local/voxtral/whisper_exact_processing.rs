// SPDX-License-Identifier: GPL-3.0-only
use anyhow::Result;
use candle_core::{Device, Tensor};
use rustfft::{FftPlanner, num_complex::Complex};
use std::f32::consts::PI;

/// Load Whisper mel filters from the same binary files
fn load_whisper_mel_filters(n_mels: usize, n_freqs: usize) -> Result<Vec<f32>> {
    use byteorder::{LittleEndian, ReadBytesExt};
    use std::io::Cursor;

    // Use the 128-mel filter bank
    let mel_bytes = include_bytes!("../data/melfilters128.bytes");

    let mut filters = vec![0f32; mel_bytes.len() / 4];
    let mut cursor = Cursor::new(mel_bytes);
    cursor.read_f32_into::<LittleEndian>(&mut filters)?;

    // The filters are stored as [n_freqs, n_mels] but we need to transpose for matrix multiply
    // mel_filters.T @ magnitudes means we want [n_mels, n_freqs] layout for efficient access
    let mut transposed = vec![0.0f32; n_mels * n_freqs];
    for mel_idx in 0..n_mels {
        for freq_idx in 0..n_freqs {
            transposed[freq_idx * n_mels + mel_idx] = filters[mel_idx * n_freqs + freq_idx];
        }
    }

    Ok(transposed)
}

/// Compute STFT frames using rustfft for better precision
fn compute_stft_frames(
    audio: &[f32],
    n_fft: usize,
    hop_length: usize,
    window: &[f32],
) -> Vec<Vec<(f32, f32)>> {
    // Match transformers audio_utils.py frame calculation exactly:
    // The audio is already padded with center=True padding, so:
    // num_frames = int(1 + np.floor((padded_waveform.size - frame_length) / hop_length))
    // Since padded_waveform.size = original_size + frame_length, this becomes:
    // num_frames = int(1 + np.floor(original_size / hop_length))
    // But we receive the already-padded audio, so we need to reverse the padding calculation

    // The input audio has been padded with n_fft/2 on each side, so:
    // original_size = audio.len() - n_fft
    let original_size = audio.len() - n_fft;
    let n_frames = 1 + original_size / hop_length;

    // Create FFT planner
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(n_fft);

    let mut stft_frames = Vec::with_capacity(n_frames);

    for frame_idx in 0..n_frames {
        let start = frame_idx * hop_length;

        // Extract windowed frame and convert to complex
        let mut complex_frame: Vec<Complex<f32>> = Vec::with_capacity(n_fft);
        for i in 0..n_fft {
            let sample = if start + i < audio.len() {
                audio[start + i] * window[i]
            } else {
                0.0
            };
            complex_frame.push(Complex::new(sample, 0.0));
        }

        // Compute FFT
        fft.process(&mut complex_frame);

        // PyTorch STFT with normalized=False (default) applies NO scaling
        // Use raw FFT values directly to match PyTorch exactly

        // Convert to (real, imag) tuples, keeping only positive frequencies
        let mut fft_result = Vec::with_capacity(n_fft / 2 + 1);
        for c in complex_frame.iter().take(n_fft / 2 + 1) {
            // Use raw FFT values without scaling to match PyTorch default behavior
            fft_result.push((c.re, c.im));
        }

        stft_frames.push(fft_result);
    }

    stft_frames
}

/// Apply reflection padding like librosa center=True, `pad_mode`='reflect'
fn apply_reflection_padding(audio: &[f32], n_fft: usize) -> Vec<f32> {
    let pad_size = n_fft / 2;
    let mut padded = Vec::with_capacity(audio.len() + 2 * pad_size);

    // Left reflection padding
    for i in 0..pad_size {
        let reflect_idx = pad_size - i - 1;
        if reflect_idx < audio.len() {
            padded.push(audio[reflect_idx]);
        } else {
            padded.push(0.0);
        }
    }

    // Original audio
    padded.extend_from_slice(audio);

    // Right reflection padding
    for i in 0..pad_size {
        let reflect_idx = audio.len() - 1 - i;
        if reflect_idx < audio.len() {
            padded.push(audio[reflect_idx]);
        } else {
            padded.push(0.0);
        }
    }

    padded
}

/// Create Hann window matching `PyTorch`'s `torch.hann_window(n_fft`, periodic=True) exactly
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn create_hann_window(n_fft: usize) -> Vec<f32> {
    // PyTorch's torch.hann_window uses periodic=True by default, which means:
    // w[n] = 0.5 * (1 - cos(2*pi*n/N)) for n = 0, 1, ..., N-1
    // This is different from symmetric (periodic=False) which uses N-1 in denominator
    (0..n_fft)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / n_fft as f32).cos()))
        .collect()
}

/// Exact implementation of `WhisperFeatureExtractor` mel spectrogram processing
/// Matches the torch implementation step by step
///
/// # Errors
///
/// Returns an error if any tensor creation or FFT operation fails or if
/// mel filter loading fails.
pub fn pcm_to_mel(audio: &[f32], device: &Device) -> Result<Tensor> {
    const N_FFT: usize = 400;
    const HOP_LENGTH: usize = 160;
    const N_MELS: usize = 128;

    // STFT with center=True (reflection padding)
    let padded_audio = apply_reflection_padding(audio, N_FFT);

    // Create Hann window
    let window = create_hann_window(N_FFT);

    // Compute STFT frames
    let stft_frames = compute_stft_frames(&padded_audio, N_FFT, HOP_LENGTH, &window);

    // // PyTorch STFT produces exactly n-1 frames compared to our calculation
    // // This is due to differences in how the last frame is handled
    // // Remove the last frame to match PyTorch exactly for magnitude calculation
    // if stft_frames.len() > 1 {
    //     stft_frames.pop();
    // }

    let n_freqs = N_FFT / 2 + 1; // 201 frequency bins (keep all including Nyquist)
    let n_frames = stft_frames.len();

    let mut power_spec = Vec::with_capacity(stft_frames.len() * n_freqs);
    for frame in &stft_frames {
        // Use all frequency bins (Python keeps all 201 frequencies)
        for &(real, imag) in frame {
            power_spec.push(real * real + imag * imag);
        }
    }
    let power_spec = power_spec;

    let mel_filters = load_whisper_mel_filters(N_MELS, n_freqs)?;

    let mut mel_spec = vec![0.0f32; n_frames * N_MELS];
    for frame_idx in 0..n_frames {
        for mel_idx in 0..N_MELS {
            let mut mel_energy = 0.0f32;
            for freq_idx in 0..n_freqs {
                mel_energy += power_spec[frame_idx * n_freqs + freq_idx]
                    * mel_filters[freq_idx * N_MELS + mel_idx];
            }
            mel_spec[frame_idx * N_MELS + mel_idx] = mel_energy;
        }
    }
    for value in &mut mel_spec {
        *value = value.max(1e-10).log10();
    }

    // Clipping (max - 8.0)
    let max_val = mel_spec.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
    let clip_min = max_val - 8.0;

    for value in &mut mel_spec {
        *value = (value.max(clip_min) + 4.0) / 4.0;
    }

    // Convert to tensor: [n_mels, n_frames] -> [1, n_mels, n_frames]
    let tensor = Tensor::from_vec(mel_spec, (n_frames, N_MELS), device)?;
    let tensor = tensor.t()?.unsqueeze(0)?; // Transpose and add batch dimension

    // Return the full tensor - let model.rs handle the chunking to match Python exactly
    Ok(tensor)
}
