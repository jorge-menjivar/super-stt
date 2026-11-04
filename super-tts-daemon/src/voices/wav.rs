// SPDX-License-Identifier: GPL-3.0-only
//! WAV in, canonical mono PCM out.
//!
//! The voice library stores one clip per voice in a single shape — mono,
//! 16-bit, [`CLIP_SAMPLE_RATE`](super::CLIP_SAMPLE_RATE) — so every backend
//! that registers a cloned voice receives the same thing and no backend needs
//! a resampler of its own. This module is the only place that knows what a
//! container looks like.
//!
//! **WAV only, deliberately.** Decoding mp3/m4a/ogg means a media parser
//! running in a daemon that lives in the user's session for the whole login,
//! and every such format is one dependency away rather than one design away:
//! `decode` is the single seam a decoder would slot into. Recording a sample
//! produces PCM already, so the path that matters most needs no decoder at
//! all.

use hound::{SampleFormat, WavSpec, WavWriter};
use std::io::Cursor;

/// A decoded clip: mono `f32` samples in `[-1.0, 1.0]`, at its own rate.
#[derive(Debug, Clone)]
pub struct Decoded {
    /// Interleaving is moot — the channels have already been mixed down.
    pub samples: Vec<f32>,
    /// The rate the file declared, before any resampling.
    pub sample_rate: u32,
}

impl Decoded {
    /// Clip length in seconds.
    #[must_use]
    pub fn seconds(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        crate::num_cast::usize_to_f32(self.samples.len())
            / crate::num_cast::u32_to_f32(self.sample_rate)
    }
}

/// Why a clip could not be read.
#[derive(Debug, thiserror::Error)]
pub enum WavError {
    /// The bytes are not a WAV file this build can read.
    #[error("{0}")]
    Unreadable(String),
    /// A WAV whose header describes something unusable (no channels, no rate,
    /// or a sample width outside 8/16/24/32-bit).
    #[error("{0}")]
    Unsupported(String),
    /// The clip is longer than the caller's bound. Reported from the header,
    /// before any samples are decoded, so an oversized file costs nothing.
    #[error("clip is {seconds:.1}s, longer than the {max:.0}s limit")]
    TooLong {
        /// Length the header describes.
        seconds: f32,
        /// The bound that was exceeded.
        max: f32,
    },
}

/// Decode a WAV file to mono `f32`, rejecting anything longer than
/// `max_seconds` before it is read.
///
/// Multi-channel input is averaged rather than left-channel-picked: a
/// reference clip recorded in stereo carries the same voice in both channels,
/// and dropping one throws away half the signal-to-noise for no reason.
///
/// # Errors
/// See [`WavError`].
pub fn decode(bytes: &[u8], max_seconds: f32) -> Result<Decoded, WavError> {
    let mut reader = hound::WavReader::new(Cursor::new(bytes))
        .map_err(|e| WavError::Unreadable(format!("not a readable WAV file: {e}")))?;
    let spec = reader.spec();

    if spec.channels == 0 || spec.sample_rate == 0 {
        return Err(WavError::Unsupported(
            "WAV header declares no channels or a zero sample rate".into(),
        ));
    }
    // The header knows the length, so the bound is enforced before the samples
    // are pulled into memory.
    let frames = reader.duration();
    let seconds =
        crate::num_cast::u32_to_f32(frames) / crate::num_cast::u32_to_f32(spec.sample_rate);
    if seconds > max_seconds {
        return Err(WavError::TooLong {
            seconds,
            max: max_seconds,
        });
    }

    let interleaved: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| WavError::Unreadable(format!("reading float samples: {e}")))?,
        // hound normalizes every integer width into `i32` for us, including
        // the unsigned 8-bit case, so one arm covers 8/16/24/32-bit files.
        (SampleFormat::Int, bits @ (8 | 16 | 24 | 32)) => {
            let full_scale = 2.0_f32.powi(i32::from(bits) - 1);
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| crate::num_cast::i32_to_f32(v) / full_scale))
                .collect::<Result<_, _>>()
                .map_err(|e| WavError::Unreadable(format!("reading integer samples: {e}")))?
        }
        (_, bits) => {
            return Err(WavError::Unsupported(format!(
                "unsupported WAV sample width: {bits} bits"
            )));
        }
    };

    let channels = usize::from(spec.channels);
    let samples = if channels == 1 {
        interleaved
    } else {
        let scale = crate::num_cast::usize_to_f32(channels);
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / scale)
            .collect()
    };

    Ok(Decoded {
        samples,
        sample_rate: spec.sample_rate,
    })
}

/// Quantize mono `f32` samples to little-endian 16-bit PCM.
///
/// Clamping before the cast is what keeps a clip that was mastered a little
/// hot from wrapping to the opposite polarity — the one conversion bug in this
/// path that is inaudible in a spectrogram and unmistakable to an ear.
#[must_use]
pub fn to_s16le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let scaled = (s * f32::from(i16::MAX)).clamp(f32::from(i16::MIN), f32::from(i16::MAX));
        out.extend_from_slice(&crate::num_cast::f32_to_i16(scaled).to_le_bytes());
    }
    out
}

/// Wrap mono `f32` samples in a 16-bit WAV file at `sample_rate`.
///
/// # Errors
/// Returns an error if the WAV writer fails, which for an in-memory buffer
/// means only that the samples could not be encoded.
pub fn encode_s16(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>, WavError> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = WavWriter::new(&mut cursor, spec)
            .map_err(|e| WavError::Unreadable(format!("starting WAV writer: {e}")))?;
        for &s in samples {
            let scaled = (s * f32::from(i16::MAX)).clamp(f32::from(i16::MIN), f32::from(i16::MAX));
            writer
                .write_sample(crate::num_cast::f32_to_i16(scaled))
                .map_err(|e| WavError::Unreadable(format!("writing sample: {e}")))?;
        }
        writer
            .finalize()
            .map_err(|e| WavError::Unreadable(format!("finalizing WAV: {e}")))?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{decode, encode_s16, to_s16le};

    /// A 16-bit WAV of `frames` frames at `rate`, `channels` wide, where every
    /// channel carries the same ramp — so a correct downmix returns the ramp
    /// unchanged and a broken one shifts its amplitude.
    fn wav(rate: u32, channels: u16, frames: u32) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for i in 0..frames {
                let v = i16::try_from(i % 1000).unwrap();
                for _ in 0..channels {
                    w.write_sample(v).unwrap();
                }
            }
            w.finalize().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn decodes_mono_and_reports_its_length() {
        let d = decode(&wav(24_000, 1, 12_000), 30.0).expect("a plain mono WAV decodes");
        assert_eq!(d.sample_rate, 24_000);
        assert_eq!(d.samples.len(), 12_000);
        assert!(
            (d.seconds() - 0.5).abs() < 1e-3,
            "half a second: {}",
            d.seconds()
        );
    }

    /// Averaging, not channel-picking: identical channels must come back at
    /// their original amplitude.
    #[test]
    fn downmixes_by_averaging() {
        let stereo = decode(&wav(24_000, 2, 100), 30.0).expect("stereo decodes");
        let mono = decode(&wav(24_000, 1, 100), 30.0).expect("mono decodes");
        assert_eq!(
            stereo.samples.len(),
            100,
            "one sample per frame, not per channel"
        );
        for (a, b) in stereo.samples.iter().zip(&mono.samples) {
            assert!((a - b).abs() < 1e-6, "{a} != {b}");
        }
    }

    /// The length bound is read off the header, so an over-long file is
    /// refused rather than decoded and then discarded.
    #[test]
    fn refuses_a_clip_past_the_bound() {
        let err = decode(&wav(24_000, 1, 24_000 * 5), 2.0).expect_err("5s is past a 2s bound");
        assert!(
            matches!(err, super::WavError::TooLong { .. }),
            "expected TooLong, got {err}"
        );
    }

    #[test]
    fn rejects_bytes_that_are_not_a_wav() {
        assert!(decode(b"ID3\x04not audio at all", 30.0).is_err());
    }

    /// Round-tripping keeps the samples: what the library stores is what a
    /// later `GET .../audio` hands back.
    #[test]
    fn encodes_what_it_decoded() {
        let original = decode(&wav(24_000, 1, 500), 30.0).unwrap();
        let bytes = encode_s16(&original.samples, 24_000).expect("encodes");
        let again = decode(&bytes, 30.0).expect("decodes its own output");
        assert_eq!(again.sample_rate, 24_000);
        assert_eq!(again.samples.len(), original.samples.len());
        for (a, b) in again.samples.iter().zip(&original.samples) {
            assert!((a - b).abs() < 1e-3, "{a} != {b}");
        }
    }

    /// Hot samples clamp to full scale instead of wrapping to the opposite
    /// polarity, which is the failure that would be audible as a click.
    #[test]
    fn clamps_rather_than_wrapping() {
        let pcm = to_s16le(&[2.0, -2.0]);
        assert_eq!(pcm, [0xFF, 0x7F, 0x00, 0x80], "clamped to +/- full scale");
    }
}
