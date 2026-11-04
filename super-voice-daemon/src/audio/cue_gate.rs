// SPDX-License-Identifier: GPL-3.0-only
//! Holding back what the microphone hears while the start cue plays.
//!
//! The recorder opens the microphone before it plays the start cue, so the
//! input stream's startup overlaps the cue instead of following it. Until the
//! cue's end is known, captured audio is held, stamped with when it was
//! captured. Once it is known, the next input callback releases what was
//! captured after the cue ended and lets audio through from then on, so a take
//! starts as the cue stops and holds none of it.

use parking_lot::Mutex;
use std::time::{Duration, Instant};

/// Mono samples, stamped with when the first of them was captured.
struct Held {
    captured_at: Instant,
    samples: Vec<f32>,
}

enum State {
    /// The cue may still be playing: hold everything, and trim it to what was
    /// captured after `cue_end` once that is known.
    Holding {
        held: Vec<Held>,
        cue_end: Option<Instant>,
    },
    /// Audio passes straight through.
    Open,
}

pub struct CueGate {
    state: Mutex<State>,
}

impl Default for CueGate {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::holding()),
        }
    }
}

impl CueGate {
    /// Hold audio again, for a new take.
    pub fn hold(&self) {
        *self.state.lock() = State::holding();
    }

    /// The cue finished playing at `cue_end`: audio captured from then on
    /// passes, and audio captured before it is dropped.
    pub fn release(&self, cue_end: Instant) {
        if let State::Holding { cue_end: end, .. } = &mut *self.state.lock() {
            *end = Some(cue_end);
        }
    }

    /// Hand `samples`, captured at `sample_rate` starting at `captured_at`, to
    /// `process`, after any held audio they release, in capture order. Holds
    /// them instead until the cue's end is known.
    pub fn admit(
        &self,
        captured_at: Instant,
        samples: Vec<f32>,
        sample_rate: u32,
        mut process: impl FnMut(Vec<f32>),
    ) {
        let mut state = self.state.lock();
        match std::mem::replace(&mut *state, State::Open) {
            State::Open => process(samples),
            State::Holding {
                mut held,
                cue_end: None,
            } => {
                held.push(Held {
                    captured_at,
                    samples,
                });
                *state = State::Holding {
                    held,
                    cue_end: None,
                };
            }
            State::Holding {
                held,
                cue_end: Some(cue_end),
            } => {
                let current = Held {
                    captured_at,
                    samples,
                };
                for chunk in held.into_iter().chain(std::iter::once(current)) {
                    if let Some(kept) = captured_after(cue_end, chunk, sample_rate) {
                        process(kept);
                    }
                }
            }
        }
    }
}

impl State {
    fn holding() -> Self {
        Self::Holding {
            held: Vec::new(),
            cue_end: None,
        }
    }
}

/// The samples of `chunk` captured at or after `cue_end`, or `None` when all
/// of them came before it.
fn captured_after(cue_end: Instant, chunk: Held, sample_rate: u32) -> Option<Vec<f32>> {
    let early = cue_end.saturating_duration_since(chunk.captured_at);
    let mut samples = chunk.samples;
    let skip = frames_in(early, sample_rate);
    if skip >= samples.len() {
        return None;
    }
    samples.drain(..skip);
    Some(samples)
}

/// How many frames at `sample_rate` start within `duration`.
fn frames_in(duration: Duration, sample_rate: u32) -> usize {
    let nanos = duration.as_nanos() * u128::from(sample_rate);
    usize::try_from(nanos.div_ceil(1_000_000_000)).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 1_000;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// `n` samples whose values count up from `from`, so a test can tell
    /// which ones were kept.
    fn samples(from: u16, n: u16) -> Vec<f32> {
        (from..from + n).map(f32::from).collect()
    }

    fn admit(gate: &CueGate, captured_at: Instant, chunk: Vec<f32>) -> Vec<Vec<f32>> {
        let mut processed = Vec::new();
        gate.admit(captured_at, chunk, RATE, |kept| processed.push(kept));
        processed
    }

    /// Nothing reaches the take while the cue may still be playing.
    #[test]
    fn audio_is_held_until_the_cue_has_played() {
        let gate = CueGate::default();
        let t0 = Instant::now();
        assert!(admit(&gate, t0, samples(0, 100)).is_empty());
        assert!(admit(&gate, t0 + ms(100), samples(100, 100)).is_empty());
    }

    /// Releasing keeps exactly what was captured from the cue's end on: a
    /// chunk wholly before it is dropped, one it falls inside is trimmed, and
    /// the chunk that triggered the release follows.
    #[test]
    fn release_keeps_what_was_captured_after_the_cue() {
        let gate = CueGate::default();
        let t0 = Instant::now();
        admit(&gate, t0, samples(0, 100));
        admit(&gate, t0 + ms(100), samples(100, 100));

        gate.release(t0 + ms(150));

        assert_eq!(
            admit(&gate, t0 + ms(200), samples(200, 100)),
            vec![samples(150, 50), samples(200, 100)]
        );
    }

    /// Once released, audio passes straight through.
    #[test]
    fn a_released_gate_passes_audio_through() {
        let gate = CueGate::default();
        let t0 = Instant::now();
        gate.release(t0);
        admit(&gate, t0, samples(0, 100));
        assert_eq!(
            admit(&gate, t0 + ms(500), samples(500, 100)),
            vec![samples(500, 100)]
        );
    }

    /// A take with no cue releases at the moment the microphone opened, so
    /// nothing it captured is lost.
    #[test]
    fn releasing_before_the_first_capture_keeps_everything() {
        let gate = CueGate::default();
        let opened = Instant::now();
        admit(&gate, opened + ms(10), samples(0, 100));
        gate.release(opened);
        assert_eq!(
            admit(&gate, opened + ms(110), samples(100, 100)),
            vec![samples(0, 100), samples(100, 100)]
        );
    }

    /// A new take holds again, without the last take's audio.
    #[test]
    fn hold_starts_over() {
        let gate = CueGate::default();
        let t0 = Instant::now();
        admit(&gate, t0, samples(0, 100));
        gate.hold();
        gate.release(t0 + ms(200));
        assert_eq!(
            admit(&gate, t0 + ms(200), samples(200, 100)),
            vec![samples(200, 100)]
        );
    }
}
