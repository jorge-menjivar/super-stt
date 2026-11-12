// SPDX-License-Identifier: GPL-3.0-only
//! The daemon's one microphone recording: whether there is one, how far it
//! has got, and how to stop it.
//!
//! This used to be three records that had to agree: a `busy` flag, the stop
//! channel, and the reservation of a recording waiting for its model. Each was
//! claimed and cleared in a different place, and they did not agree:
//!
//! - The stop channel was installed only once the recorder was built, a second
//!   or so after `busy`, so a stop in between reached nothing and the
//!   microphone opened anyway.
//! - The reservation had no owner. A second recording could take it, or clear
//!   it, and the first then failed its own claim and cleared the second's
//!   `busy` under its live take.
//! - A recording dropped while it waited kept its reservation, so every start
//!   was refused until the daemon restarted.
//!
//! A [`Claim`] is the one record now. It belongs to the recording that made it,
//! it carries the stop channel from the moment the recording holds the
//! microphone, and dropping it, however the recording ends, frees the
//! microphone.

use super::RecordingCancelled;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use tokio::sync::{broadcast, oneshot};

/// Where the recording is, if there is one.
#[derive(Debug, Default)]
pub(crate) struct RecordingState {
    phase: Mutex<Phase>,
    next_id: AtomicU64,
}

#[derive(Debug, Default)]
enum Phase {
    #[default]
    Idle,
    /// Waiting for its model to load, nothing captured yet. `cancel` is taken
    /// by the stop that cancels it.
    Waiting {
        id: u64,
        cancel: Option<oneshot::Sender<()>>,
    },
    /// Holds the microphone: building the recorder, capturing, or
    /// transcribing what it captured. `stop` reaches the recorder until
    /// capture ends.
    Active {
        id: u64,
        stop: Option<broadcast::Sender<()>>,
    },
}

impl Phase {
    fn id(&self) -> Option<u64> {
        match self {
            Self::Idle => None,
            Self::Waiting { id, .. } | Self::Active { id, .. } => Some(*id),
        }
    }
}

/// What [`RecordingState::stop`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stopped {
    /// There was no recording.
    NothingToStop,
    /// The recording was waiting for its model, and will not start.
    Cancelled,
    /// The recorder was told to stop capturing.
    Signalled,
    /// Capture had ended: the take is being transcribed, which a stop does
    /// not interrupt.
    Transcribing,
}

impl RecordingState {
    /// Whether a recording is under way: waiting for its model, or holding
    /// the microphone.
    pub(crate) fn is_busy(&self) -> bool {
        !matches!(*self.phase(), Phase::Idle)
    }

    /// Whether a recording is waiting for its model to load.
    pub(crate) fn is_waiting(&self) -> bool {
        matches!(*self.phase(), Phase::Waiting { .. })
    }

    /// Whether a recording holds the microphone and its take is being
    /// transcribed, so there is no capture left to stop.
    pub(crate) fn is_transcribing(&self) -> bool {
        matches!(*self.phase(), Phase::Active { stop: None, .. })
    }

    /// Reserve the microphone for a new recording. `None` when one is under
    /// way: one at a time.
    ///
    /// The recording starts out waiting, which its model may make it do; it
    /// takes the microphone with [`Claim::start`]. The receiver resolves when
    /// a stop cancels it before then.
    pub(crate) fn reserve(&self) -> Option<(Claim<'_>, oneshot::Receiver<()>)> {
        let mut phase = self.phase();
        if !matches!(*phase, Phase::Idle) {
            return None;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (cancel, cancelled) = oneshot::channel();
        *phase = Phase::Waiting {
            id,
            cancel: Some(cancel),
        };
        Some((Claim { state: self, id }, cancelled))
    }

    /// Stop the recording: cancel one waiting for its model, or tell the
    /// recorder of one capturing to stop.
    pub(crate) fn stop(&self) -> Stopped {
        match &mut *self.phase() {
            Phase::Idle => Stopped::NothingToStop,
            // Marked rather than ended: the recording ends itself on seeing
            // the cancel, so no other can take its place before it has.
            Phase::Waiting { cancel, .. } => {
                if let Some(cancel) = cancel.take() {
                    // Unreceived when its load has just finished: the mark is
                    // what `Claim::start` then finds.
                    let _ = cancel.send(());
                }
                Stopped::Cancelled
            }
            Phase::Active {
                stop: Some(stop), ..
            } => {
                let _ = stop.send(());
                Stopped::Signalled
            }
            Phase::Active { stop: None, .. } => Stopped::Transcribing,
        }
    }

    fn phase(&self) -> MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One recording's hold on the microphone. Dropping it frees the microphone.
#[derive(Debug)]
pub(crate) struct Claim<'a> {
    state: &'a RecordingState,
    id: u64,
}

impl Claim<'_> {
    /// Take the microphone. Returns the receiver the recorder stops on, which
    /// a stop reaches from now on.
    ///
    /// # Errors
    /// [`RecordingCancelled`] when a stop cancelled the recording while it
    /// waited.
    pub(crate) fn start(&self) -> Result<broadcast::Receiver<()>, RecordingCancelled> {
        let mut phase = self.state.phase();
        match &*phase {
            Phase::Waiting {
                id,
                cancel: Some(_),
            } if *id == self.id => {
                let (stop, stopped) = broadcast::channel(1);
                *phase = Phase::Active {
                    id: self.id,
                    stop: Some(stop),
                };
                Ok(stopped)
            }
            _ => Err(RecordingCancelled),
        }
    }

    /// Capture has ended: a stop now finds the take being transcribed.
    pub(crate) fn capture_ended(&self) {
        if let Phase::Active { id, stop } = &mut *self.state.phase()
            && *id == self.id
        {
            *stop = None;
        }
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        let mut phase = self.state.phase();
        if phase.id() == Some(self.id) {
            *phase = Phase::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RecordingState, Stopped};

    /// One recording at a time, and the microphone is free again once it
    /// ends, however it ends.
    #[test]
    fn one_recording_at_a_time_until_its_claim_drops() {
        let state = RecordingState::default();
        let (claim, _cancelled) = state.reserve().expect("free");
        assert!(state.reserve().is_none(), "two recordings at once");
        assert!(state.is_busy());

        drop(claim);
        assert!(!state.is_busy());
        assert!(state.reserve().is_some());
    }

    /// A stop reaches a recording from the moment it takes the microphone,
    /// before its recorder is built.
    #[test]
    fn a_stop_reaches_a_recording_as_soon_as_it_starts() {
        let state = RecordingState::default();
        let (claim, _cancelled) = state.reserve().expect("free");
        let mut stopped = claim.start().expect("not cancelled");

        assert_eq!(state.stop(), Stopped::Signalled);
        assert!(stopped.try_recv().is_ok(), "the recorder never heard it");
    }

    /// A stop while the recording waits for its model cancels it, and the
    /// recording finds that when it would take the microphone.
    #[test]
    fn a_stop_while_waiting_cancels_the_recording() {
        let state = RecordingState::default();
        let (claim, mut cancelled) = state.reserve().expect("free");

        assert_eq!(state.stop(), Stopped::Cancelled);
        assert!(cancelled.try_recv().is_ok());
        assert!(claim.start().is_err(), "a cancelled recording started");
        assert!(state.is_busy(), "freed before the recording saw its cancel");
        drop(claim);
        assert!(!state.is_busy());
    }

    /// Once capture has ended, a stop finds the take being transcribed.
    #[test]
    fn a_stop_after_capture_finds_the_take_transcribing() {
        let state = RecordingState::default();
        let (claim, _cancelled) = state.reserve().expect("free");
        let _stopped = claim.start().expect("not cancelled");
        claim.capture_ended();

        assert!(state.is_transcribing());
        assert_eq!(state.stop(), Stopped::Transcribing);
    }

    /// A claim only ever frees its own recording: a stale one dropping late
    /// leaves the next recording alone.
    #[test]
    fn a_claim_frees_only_its_own_recording() {
        let state = RecordingState::default();
        let (first, _) = state.reserve().expect("free");
        // The first is cancelled and freed by hand, as if it had ended, while
        // a value of it lingers.
        assert_eq!(state.stop(), Stopped::Cancelled);
        let stale = super::Claim {
            state: &state,
            id: first.id,
        };
        drop(first);
        let (second, _) = state.reserve().expect("free again");

        drop(stale);
        assert!(state.is_busy(), "a stale claim freed the next recording");
        assert!(second.start().is_ok());
    }

    /// Nothing to stop when no recording is under way.
    #[test]
    fn a_stop_with_no_recording_does_nothing() {
        assert_eq!(RecordingState::default().stop(), Stopped::NothingToStop);
    }
}
