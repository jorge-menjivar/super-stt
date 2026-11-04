// SPDX-License-Identifier: GPL-3.0-only
//! Turning a backend's synthesis chunk into samples the output device can take.
//!
//! Three transformations, in order: resample to the device rate, map channels,
//! and fade each end of the chunk. All of it is pure and runs on the producer
//! side — the audio callback only ever memcpys out of the ring.
//!
//! The seam is the part that earns its keep. Consecutive chunks come from
//! *separate* synthesis requests (sentence 1, then sentence 2), so nothing makes
//! the last sample of one continuous with the first of the next. Butting them
//! together steps the waveform discontinuously, and that step is audible as a
//! click on every sentence boundary. [`SeamFade`] takes each chunk to zero at
//! both ends so the boundary is continuous by construction — see its docs for
//! why that rather than an overlap-add crossfade.

use anyhow::Result;
use super_tts_shared::audio::frames::AudioParams;
use super_tts_shared::utils::audio::{ResampleQuality, resample};

/// The output device's interleaved format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceFormat {
    /// Device sample rate in Hz.
    pub sample_rate: u32,
    /// Device channel count.
    pub channels: u16,
}

/// Spread mono samples across `channels`, or fold multi-channel down to mono.
///
/// Only the two cases the contract can produce are handled — a backend declares
/// 1 or 2 channels — and anything else falls back to using the first channel,
/// which is wrong-sounding but never panics or misaligns the interleave.
#[must_use]
pub fn map_channels(samples: &[f32], from: u16, to: u16) -> Vec<f32> {
    if from == to {
        return samples.to_vec();
    }
    if from == 1 {
        // Mono to N: duplicate. Not a pan law — speech is centered, and
        // duplicating is what every consumer expects.
        let to = to as usize;
        let mut out = Vec::with_capacity(samples.len() * to);
        for s in samples {
            out.extend(std::iter::repeat_n(*s, to));
        }
        return out;
    }
    // Multi-channel to mono: average the frame.
    let from_n = from as usize;
    let mono: Vec<f32> = samples
        .chunks(from_n)
        .map(|frame| frame.iter().sum::<f32>() / crate::num_cast::usize_to_f32(frame.len()))
        .collect();
    if to == 1 {
        mono
    } else {
        map_channels(&mono, 1, to)
    }
}

/// Resample and channel-map one chunk into the device's interleaved format.
///
/// # Errors
/// Returns an error if resampling fails.
pub fn prepare(samples: &[f32], params: AudioParams, device: DeviceFormat) -> Result<Vec<f32>> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }
    // Resample before channel mapping: doing it the other way round would run
    // the resampler over duplicated mono, which is the same answer for more
    // work, and would interpolate *across* interleaved channels for the
    // downmix case, which is simply wrong.
    let resampled = if params.sample_rate == device.sample_rate {
        samples.to_vec()
    } else {
        resample(
            samples,
            params.sample_rate,
            device.sample_rate,
            // Balanced, not Fast: this runs once per chunk on the producer
            // task, well off the audio callback, and unlike the STT capture
            // path the result is what the user actually hears.
            ResampleQuality::Balanced,
        )?
    };
    Ok(map_channels(&resampled, params.channels, device.channels))
}

/// Short fade applied to each end of every chunk, so seams meet at zero.
///
/// **Not an overlap-add crossfade, deliberately.** An overlap-add mixes the
/// tail of one chunk with the head of the next over a shared window, which is
/// correct when both sides describe the *same* stretch of time — overlapping
/// analysis windows, say. Successive synthesis chunks do not: chunk N is
/// "Hello there." and chunk N+1 is "How are you?", strictly sequential speech.
/// Overlapping them mixes the end of one word into the start of another and
/// shortens the utterance by the fade length on every seam. Losing ~15 ms of
/// speech per sentence is a real defect; a momentary dip is not.
///
/// So each chunk is faded in at its head and out at its tail, with no overlap.
/// Total length is preserved exactly, and every boundary — including the start
/// and end of the utterance — passes through zero instead of stepping.
///
/// [`Playback`](super::Playback) decides *where* the boundaries are: it fades
/// in once per synthesis chunk and holds back one fade's worth of tail so
/// [`Playback::end_chunk`](super::Playback::end_chunk) can fade it out. That
/// costs one fade of latency (8 ms) and is the price of being able to fade out
/// at all — the samples have to still be in reach when the chunk turns out to
/// be over.
///
/// The gain curve is equal-power (`sin`/`cos`) rather than linear: it reaches
/// its half-way point faster, which keeps the dip short.
#[derive(Debug, Clone, Copy)]
pub struct SeamFade {
    /// Fade length per end, in **frames**.
    fade_frames: usize,
    /// Channels per frame, so the same gain lands on every channel of a frame.
    channels: usize,
}

impl SeamFade {
    /// A fade of `fade_ms` per end at the given device format. Zero disables
    /// it and chunks are concatenated exactly as prepared.
    #[must_use]
    pub fn new(fade_ms: u32, device: DeviceFormat) -> Self {
        Self {
            fade_frames: (u64::from(fade_ms) * u64::from(device.sample_rate) / 1000) as usize,
            channels: device.channels.max(1) as usize,
        }
    }

    /// Fade length per end, in interleaved samples.
    #[must_use]
    pub const fn samples(self) -> usize {
        self.fade_frames * self.channels
    }

    /// Apply the fade to one prepared chunk, in place.
    ///
    /// Gains are computed **per frame**, not per interleaved sample: indexing a
    /// stereo buffer sample-wise gives the two channels of one frame adjacent
    /// positions on the gain curve, which attenuates left and right by
    /// different amounts and drags the stereo image off centre for the length
    /// of the fade.
    ///
    /// A chunk shorter than two fades has its fade shortened to half its length
    /// rather than skipped, so even a very short chunk still meets zero at both
    /// ends and no sample is dropped.
    pub fn apply(self, chunk: &mut [f32]) {
        self.fade_in(chunk);
        self.fade_out(chunk);
    }

    /// Fade the head of `chunk` up from zero.
    pub fn fade_in(self, chunk: &mut [f32]) {
        let Some((n, ch)) = self.span(chunk) else {
            return;
        };
        for i in 0..n {
            let gain = Self::gain(i, n);
            let head = i * ch;
            for c in 0..ch {
                chunk[head + c] *= gain;
            }
        }
    }

    /// Fade the tail of `chunk` down to zero.
    pub fn fade_out(self, chunk: &mut [f32]) {
        let Some((n, ch)) = self.span(chunk) else {
            return;
        };
        let frames = chunk.len() / ch;
        for i in 0..n {
            let gain = Self::gain(i, n);
            let tail = (frames - 1 - i) * ch;
            for c in 0..ch {
                chunk[tail + c] *= gain;
            }
        }
    }

    /// `(fade frames, channels)` for `chunk`, or `None` when there is nothing
    /// to do. The fade is capped at half the chunk so a head and a tail fade on
    /// the same buffer never overlap.
    fn span(self, chunk: &[f32]) -> Option<(usize, usize)> {
        if self.fade_frames == 0 || chunk.is_empty() {
            return None;
        }
        let ch = self.channels;
        let frames = chunk.len() / ch;
        if frames == 0 {
            return None;
        }
        Some((self.fade_frames.min(frames / 2).max(1).min(frames), ch))
    }

    /// Gain at step `i` of an `n`-frame fade.
    fn gain(i: usize, n: usize) -> f32 {
        let t = (crate::num_cast::usize_to_f32(i) + 0.5) / crate::num_cast::usize_to_f32(n);
        equal_power(t).0
    }
}

/// Equal-power gain pair at position `t` in `[0, 1]`: `(fade_in, fade_out)`.
/// Their squares sum to 1, so a fade holds constant power rather than dipping
/// ~3 dB in the middle the way a linear ramp does.
fn equal_power(t: f32) -> (f32, f32) {
    let angle = t * std::f32::consts::FRAC_PI_2;
    (angle.sin(), angle.cos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super_tts_shared::audio::frames::SampleFormat;

    fn params(sample_rate: u32, channels: u16) -> AudioParams {
        AudioParams {
            sample_rate,
            channels,
            format: SampleFormat::F32Le,
        }
    }

    const DEVICE: DeviceFormat = DeviceFormat {
        sample_rate: 48000,
        channels: 2,
    };

    #[test]
    fn mono_is_duplicated_across_channels() {
        assert_eq!(map_channels(&[1.0, 2.0], 1, 2), vec![1.0, 1.0, 2.0, 2.0]);
        assert_eq!(map_channels(&[1.0], 1, 1), vec![1.0]);
    }

    #[test]
    fn stereo_folds_down_by_averaging_frames() {
        assert_eq!(map_channels(&[1.0, 3.0, 0.0, 2.0], 2, 1), vec![2.0, 1.0]);
    }

    #[test]
    fn prepare_resamples_and_maps_in_one_pass() {
        // 24 kHz mono in, 48 kHz stereo out: twice the frames, twice the
        // channels, so four times the interleaved samples (within the
        // resampler's edge handling).
        let input: Vec<f32> = (0..480).map(|i| (i as f32 / 48.0).sin()).collect();
        let out = prepare(&input, params(24000, 1), DEVICE).expect("prepare");
        let frames = out.len() / 2;
        assert!(
            (frames as i64 - 960).abs() < 64,
            "expected ~960 frames, got {frames}"
        );
        assert_eq!(out.len() % 2, 0, "output must stay frame-aligned");
    }

    #[test]
    fn prepare_passes_a_matching_format_through_untouched() {
        let input = vec![0.1, -0.2, 0.3, -0.4];
        let out = prepare(&input, params(48000, 2), DEVICE).expect("prepare");
        assert_eq!(out, input);
    }

    #[test]
    fn an_empty_chunk_prepares_to_nothing() {
        assert!(prepare(&[], params(24000, 1), DEVICE).unwrap().is_empty());
    }

    const KHZ_MONO: DeviceFormat = DeviceFormat {
        sample_rate: 1000,
        channels: 1,
    };

    fn max_step(samples: &[f32]) -> f32 {
        samples
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0_f32, f32::max)
    }

    /// The seam is what this exists for: two chunks that sit at opposite DC
    /// levels must join without the 2.0 step surviving.
    #[test]
    fn fading_removes_the_step_between_two_chunks() {
        let fade = SeamFade::new(20, KHZ_MONO); // 20 samples at 1 kHz mono
        let mut a = vec![1.0_f32; 100];
        let mut b = vec![-1.0_f32; 100];
        fade.apply(&mut a);
        fade.apply(&mut b);

        let mut out = a;
        out.extend(b);
        assert_eq!(out.len(), 200, "no samples are lost across the seam");
        let step = max_step(&out);
        assert!(step < 0.2, "the 2.0 step must be smoothed; got {step}");
    }

    /// Without a fade the same input keeps its discontinuity — the control that
    /// proves the assertion above measures something.
    #[test]
    fn without_a_fade_the_step_survives() {
        let fade = SeamFade::new(0, KHZ_MONO);
        let mut a = vec![1.0_f32; 100];
        let mut b = vec![-1.0_f32; 100];
        fade.apply(&mut a);
        fade.apply(&mut b);
        let mut out = a;
        out.extend(b);
        assert!(
            (max_step(&out) - 2.0).abs() < 1e-6,
            "expected the raw 2.0 step"
        );
    }

    /// Content preservation is the property that rules out overlap-add: the
    /// utterance must not get shorter every time it is split into more chunks.
    #[test]
    fn fading_preserves_length_however_the_audio_is_chunked() {
        let fade = SeamFade::new(20, KHZ_MONO);
        for chunk_len in [30_usize, 50, 100, 300] {
            let total: usize = (0..6)
                .map(|_| {
                    let mut c = vec![0.5_f32; chunk_len];
                    fade.apply(&mut c);
                    c.len()
                })
                .sum();
            assert_eq!(total, chunk_len * 6, "no samples lost at {chunk_len}/chunk");
        }
    }

    #[test]
    fn both_ends_of_a_chunk_reach_zero() {
        let fade = SeamFade::new(20, KHZ_MONO);
        let mut c = vec![1.0_f32; 100];
        fade.apply(&mut c);
        assert!(c[0] < 0.1, "head starts near zero, got {}", c[0]);
        assert!(c[99] < 0.1, "tail ends near zero, got {}", c[99]);
        assert!(
            (c[50] - 1.0).abs() < 1e-6,
            "the middle is untouched, got {}",
            c[50]
        );
    }

    /// A chunk shorter than two fades must still be faded at both ends and keep
    /// every sample, rather than being skipped or truncated.
    #[test]
    fn a_chunk_shorter_than_two_fades_is_still_faded_and_kept_whole() {
        let fade = SeamFade::new(20, KHZ_MONO); // 20 samples
        let mut c = vec![1.0_f32; 6];
        fade.apply(&mut c);
        assert_eq!(c.len(), 6);
        assert!(c[0] < 1.0 && c[5] < 1.0, "both ends are attenuated");
        assert!(c.iter().all(|s| s.is_finite()));
    }

    /// Regression: the gain has to be per frame. Indexing a stereo buffer
    /// sample-wise puts the two channels of one frame at adjacent points on the
    /// curve, so they come out at different levels and the image drifts off
    /// centre for the length of the fade.
    #[test]
    fn a_stereo_fade_attenuates_both_channels_of_a_frame_equally() {
        let stereo = DeviceFormat {
            sample_rate: 1000,
            channels: 2,
        };
        let fade = SeamFade::new(20, stereo);
        // 100 frames of a centred signal: both channels identical throughout.
        let mut c = vec![1.0_f32; 200];
        fade.apply(&mut c);
        for (i, frame) in c.chunks(2).enumerate() {
            assert!(
                (frame[0] - frame[1]).abs() < 1e-6,
                "frame {i} split apart: {} vs {}",
                frame[0],
                frame[1]
            );
        }
        assert!(c[0] < 1.0 && c[199] < 1.0, "both ends are still faded");
        assert!((c[100] - 1.0).abs() < 1e-6, "the middle is untouched");
    }

    #[test]
    fn a_single_sample_chunk_does_not_panic() {
        let fade = SeamFade::new(20, KHZ_MONO);
        let mut c = vec![1.0_f32];
        fade.apply(&mut c);
        assert_eq!(c.len(), 1);
        assert!(c[0].is_finite());
    }

    #[test]
    fn an_empty_chunk_is_left_alone() {
        let fade = SeamFade::new(20, KHZ_MONO);
        let mut c: Vec<f32> = Vec::new();
        fade.apply(&mut c);
        assert!(c.is_empty());
    }

    #[test]
    fn equal_power_gains_hold_constant_power() {
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let (a, b) = equal_power(t);
            assert!(
                (a * a + b * b - 1.0).abs() < 1e-5,
                "power must stay 1.0 at t={t}"
            );
        }
    }
}
