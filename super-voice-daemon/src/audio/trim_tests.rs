// SPDX-License-Identifier: GPL-3.0-only
use super::trim_trailing_silence;

const RATE: u32 = 16_000;
const THRESHOLD: f32 = 0.01;

fn seconds(secs: f64, amplitude: f32) -> Vec<f32> {
    // reason: test lengths are small and exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = (secs * f64::from(RATE)) as usize;
    // Alternate the sign so the signal has the RMS of its amplitude without
    // being a DC offset.
    (0..n)
        .map(|i| if i % 2 == 0 { amplitude } else { -amplitude })
        .collect()
}

fn take(parts: &[Vec<f32>]) -> Vec<f32> {
    parts.concat()
}

/// Speech, then two seconds of room noise with a key click in it: the tail
/// goes, the speech and a short pad stay.
#[test]
fn a_silent_tail_with_a_click_is_cut_after_the_last_speech() {
    let audio = take(&[
        seconds(1.0, 0.1),
        seconds(1.0, 0.001),
        seconds(0.01, 0.5), // the stop key
        seconds(1.0, 0.001),
    ]);
    let kept = trim_trailing_silence(&audio, RATE, THRESHOLD);
    // One second of speech plus the 300 ms pad.
    assert_eq!(kept.len(), 16_000 + 4_800);
}

/// A take that ends while the user is still talking is left alone.
#[test]
fn a_take_ending_in_speech_is_unchanged() {
    let audio = take(&[seconds(0.5, 0.001), seconds(1.0, 0.1)]);
    let kept = trim_trailing_silence(&audio, RATE, THRESHOLD);
    assert_eq!(kept.len(), audio.len());
}

/// No speech at all: nothing to anchor a cut on, so nothing is cut. The
/// no-speech path upstream never decodes such a take anyway.
#[test]
fn a_take_with_no_speech_is_unchanged() {
    let audio = seconds(3.0, 0.001);
    let kept = trim_trailing_silence(&audio, RATE, THRESHOLD);
    assert_eq!(kept.len(), audio.len());
}

/// A tail shorter than the pad plus the minimum worth cutting stays: the
/// decoder gets the same audio it always did.
#[test]
fn a_short_tail_is_unchanged() {
    let audio = take(&[seconds(1.0, 0.1), seconds(0.4, 0.001)]);
    let kept = trim_trailing_silence(&audio, RATE, THRESHOLD);
    assert_eq!(kept.len(), audio.len());
}

/// A click alone is a transient, not speech: it must not anchor the cut.
#[test]
fn a_click_alone_does_not_count_as_speech() {
    let audio = take(&[
        seconds(1.0, 0.1),
        seconds(2.0, 0.001),
        seconds(0.02, 0.5),
        seconds(2.0, 0.001),
    ]);
    let kept = trim_trailing_silence(&audio, RATE, THRESHOLD);
    assert_eq!(kept.len(), 16_000 + 4_800);
}
