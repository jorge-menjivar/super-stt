// SPDX-License-Identifier: GPL-3.0-only

//! Trim the non-speech tail off a finished take before the final decode.
//!
//! A take ends when the user presses the stop shortcut or falls silent, so
//! its last second is a breath, the click of the key, and room noise. A
//! batch model asked to transcribe that invents words for it: whisper's
//! favourites are "you" and "Thank you.", and one take ended "…everything
//! works. you". The previews never see this tail — near-silent windows are
//! skipped — so only the final transcript carried it.

/// Loudness is measured over frames this long.
const FRAME: std::time::Duration = std::time::Duration::from_millis(20);

/// Consecutive loud frames that count as speech rather than a transient. A
/// key click is loud for a frame or two; a syllable lasts longer.
const SPEECH_RUN_FRAMES: usize = 5;

/// Audio kept after the last speech, so a final consonant is not clipped.
const PAD: std::time::Duration = std::time::Duration::from_millis(300);

/// A tail shorter than this is not worth cutting.
const MIN_TRIM: std::time::Duration = std::time::Duration::from_millis(200);

/// `samples` without its trailing non-speech, or unchanged when the take
/// ends in speech, holds no speech at all, or the tail is too short to
/// matter. `threshold` is the RMS above which a frame counts as loud; the
/// recorder's adaptive speech threshold is the one to pass.
#[must_use]
pub fn trim_trailing_silence(samples: &[f32], sample_rate: u32, threshold: f32) -> &[f32] {
    let per_second = usize::try_from(sample_rate).unwrap_or(16_000).max(1);
    let frame_len = (per_second * FRAME.as_millis() as usize / 1000).max(1);
    let pad = per_second * PAD.as_millis() as usize / 1000;
    let min_trim = per_second * MIN_TRIM.as_millis() as usize / 1000;

    // End (in samples) of the last run of at least SPEECH_RUN_FRAMES loud
    // frames.
    let mut run = 0;
    let mut last_speech_end = None;
    for (i, frame) in samples.chunks(frame_len).enumerate() {
        if rms(frame) > threshold {
            run += 1;
            if run >= SPEECH_RUN_FRAMES {
                last_speech_end = Some((i + 1) * frame_len);
            }
        } else {
            run = 0;
        }
    }

    let Some(speech_end) = last_speech_end else {
        return samples;
    };
    let keep = (speech_end + pad).min(samples.len());
    if samples.len() - keep < min_trim {
        return samples;
    }
    &samples[..keep]
}

fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    // reason: a frame is a few hundred samples; the count fits an f32.
    #[allow(clippy::cast_precision_loss)]
    let mean_square = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
    mean_square.sqrt()
}

#[cfg(test)]
#[path = "trim_tests.rs"]
mod tests;
