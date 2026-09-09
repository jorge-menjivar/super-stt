// SPDX-License-Identifier: GPL-3.0-only
use std::time::Instant;

/// Where the applet believes the daemon is in a recording cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Idle,
    Recording,
    Processing,
}

impl RecordingState {
    /// Fold a `recording_state` event into the current state.
    ///
    /// The daemon publishes `recording_state{is_recording:false}` and
    /// `transcribing_started` on two different topics, and forwards every
    /// topic of a subscription on its own task, so the two frames can reach
    /// the applet in either order. When the transcribing frame wins that race
    /// the applet is already `Processing`, and reading the mic stop as "not
    /// recording, so idle" would cancel the transcribing animation for the
    /// rest of the cycle. `Processing` therefore stays put here;
    /// `transcribing_stopped` is the only event that returns us to `Idle`.
    fn with_recording_flag(self, is_recording: bool) -> Self {
        match (self, is_recording) {
            (_, true) => Self::Recording,
            (Self::Recording | Self::Processing, false) => Self::Processing,
            (Self::Idle, false) => Self::Idle,
        }
    }
}

/// One lifecycle event from the daemon's `/events` stream, as the applet's
/// cycle state machine sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleEvent {
    /// `recording_state` — mic capture started (`true`) or ended (`false`).
    RecordingFlag(bool),
    /// `transcribing_started` — model decode of the captured audio began.
    TranscribingStarted,
    /// `transcribing_stopped` — the cycle is over, success or failure.
    TranscribingStopped,
}

/// What a transition leaves for the caller to do to the drawing components,
/// which the phase itself knows nothing about.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PhaseChange {
    /// The animation clock just started, so the working-animation component
    /// has to be reset before it draws its first frame.
    pub animation_restarted: bool,
    /// Live audio has stopped, so the visualization is holding stale bars.
    pub visualization_stale: bool,
}

/// The recording cycle as the applet tracks it: which phase we are in, plus
/// the clock that drives the transcribing animation.
///
/// Each event of a cycle is a separate `/events` topic and the daemon
/// forwards every topic of a subscription on its own task, so they arrive in
/// any order. [`Self::apply`] is written to be order-insensitive, and the
/// tests drive every interleaving of a cycle through it.
#[derive(Debug)]
pub struct RecordingPhase {
    state: RecordingState,
    /// Wall-clock start of the current transcribing phase; `Some` only while
    /// the animation runs, used to derive its elapsed time.
    animation_start: Option<Instant>,
}

impl Default for RecordingPhase {
    fn default() -> Self {
        Self {
            state: RecordingState::Idle,
            animation_start: None,
        }
    }
}

impl RecordingPhase {
    /// Mic capture is live, so the applet draws the audio visualization.
    pub fn is_recording(&self) -> bool {
        self.state == RecordingState::Recording
    }

    /// The daemon is decoding, so the applet draws the working animation.
    pub fn is_transcribing(&self) -> bool {
        self.state == RecordingState::Processing
    }

    /// How long the working animation has been running, in milliseconds.
    /// `None` whenever it isn't running.
    pub fn animation_elapsed_ms(&self) -> Option<f32> {
        self.animation_start
            .map(|start| start.elapsed().as_secs_f32() * 1000.0)
    }

    /// Fold one lifecycle event in, returning the work the caller owes the
    /// drawing components.
    pub fn apply(&mut self, event: CycleEvent) -> PhaseChange {
        match event {
            CycleEvent::RecordingFlag(is_recording) => {
                // Mic capture ended, so the live bars are stale whether or
                // not `transcribing_started` already moved us to Processing.
                let visualization_stale = !is_recording && self.state != RecordingState::Idle;
                let next = self.state.with_recording_flag(is_recording);
                let mut change = self.enter(next);
                change.visualization_stale = visualization_stale;
                change
            }
            CycleEvent::TranscribingStarted => {
                // The mirror of the race handled in `with_recording_flag`:
                // only enter Processing mid-cycle, never resurrect it after
                // `transcribing_stopped` already returned us to Idle.
                if self.state == RecordingState::Idle {
                    return PhaseChange::default();
                }
                self.enter(RecordingState::Processing)
            }
            CycleEvent::TranscribingStopped => {
                let mut change = self.enter(RecordingState::Idle);
                change.visualization_stale = true;
                change
            }
        }
    }

    /// Move to `next`, starting or stopping the animation clock with it. The
    /// clock survives a Processing → Processing transition, so a second event
    /// that lands in the same phase doesn't restart the animation mid-cycle.
    fn enter(&mut self, next: RecordingState) -> PhaseChange {
        let mut change = PhaseChange::default();
        if next == RecordingState::Processing {
            if self.animation_start.is_none() {
                self.animation_start = Some(Instant::now());
                change.animation_restarted = true;
            }
        } else {
            self.animation_start = None;
        }
        self.state = next;
        change
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonConnectionState {
    Connecting,
    Connected,
    Error(String),
    /// User denied the consent prompt (or the daemon's sticky deny
    /// cache short-circuited a fresh request). The widget
    /// subscription has terminated to avoid spamming retries. The
    /// applet UI shows a hint to restart the daemon and a button
    /// that triggers `Message::RetryAuthorization`.
    Blocked(String),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum IsOpen {
    None,
    VisualizationTheme,
    WorkingAnimation,
    VisualizationColors,
}

#[cfg(test)]
mod recording_state_tests {
    use super::RecordingState;

    #[test]
    fn mic_start_always_enters_recording() {
        for state in [
            RecordingState::Idle,
            RecordingState::Recording,
            RecordingState::Processing,
        ] {
            assert_eq!(state.with_recording_flag(true), RecordingState::Recording);
        }
    }

    #[test]
    fn mic_stop_from_recording_enters_processing() {
        assert_eq!(
            RecordingState::Recording.with_recording_flag(false),
            RecordingState::Processing,
        );
    }

    #[test]
    fn mic_stop_after_transcribing_started_stays_processing() {
        // Regression guard: `transcribing_started` and
        // `recording_state{is_recording:false}` race on the wire, and when the
        // transcribing frame lands first this event used to knock the applet
        // back to `Idle`, so the transcribing animation never appeared. Which
        // way the race falls is per connection, so with two applets on the
        // panel one side would animate and the other would not.
        assert_eq!(
            RecordingState::Processing.with_recording_flag(false),
            RecordingState::Processing,
        );
    }

    #[test]
    fn mic_stop_while_idle_stays_idle() {
        assert_eq!(
            RecordingState::Idle.with_recording_flag(false),
            RecordingState::Idle,
        );
    }
}

#[cfg(test)]
mod recording_phase_tests {
    //! The daemon publishes the events of one cycle in a fixed order but
    //! forwards each topic on its own task, so the applet can see them in any
    //! order. These tests drive the orderings the wire can actually produce
    //! and pin two properties: the animation is up for the whole transcribing
    //! phase, and it is never left running once the cycle closes.
    use super::{
        CycleEvent,
        CycleEvent::{RecordingFlag, TranscribingStarted, TranscribingStopped},
        RecordingPhase,
    };

    /// The three events that close a cycle, in the order the daemon publishes
    /// them. Each is its own topic, so any permutation can reach the applet.
    const CLOSING_ORDERS: [[CycleEvent; 3]; 6] = [
        [
            RecordingFlag(false),
            TranscribingStarted,
            TranscribingStopped,
        ],
        [
            RecordingFlag(false),
            TranscribingStopped,
            TranscribingStarted,
        ],
        [
            TranscribingStarted,
            RecordingFlag(false),
            TranscribingStopped,
        ],
        [
            TranscribingStarted,
            TranscribingStopped,
            RecordingFlag(false),
        ],
        [
            TranscribingStopped,
            RecordingFlag(false),
            TranscribingStarted,
        ],
        [
            TranscribingStopped,
            TranscribingStarted,
            RecordingFlag(false),
        ],
    ];

    /// A phase that has just seen `recording_state{is_recording:true}`.
    fn recording() -> RecordingPhase {
        let mut phase = RecordingPhase::default();
        let change = phase.apply(RecordingFlag(true));
        assert!(phase.is_recording());
        assert!(!change.animation_restarted);
        phase
    }

    #[test]
    fn transcribing_animation_runs_whichever_way_the_mic_stop_race_falls() {
        // The regression this suite exists for: `transcribing_started` and
        // `recording_state{is_recording:false}` are published microseconds
        // apart on two topics, so either can arrive first. Both applets on a
        // panel hold their own `/events` connection and so race separately,
        // which is what made the animation show up on one side only.
        for order in [
            [RecordingFlag(false), TranscribingStarted],
            [TranscribingStarted, RecordingFlag(false)],
        ] {
            let mut phase = recording();
            let restarts = order
                .iter()
                .filter(|event| phase.apply(**event).animation_restarted)
                .count();

            assert!(
                phase.is_transcribing(),
                "{order:?} left the transcribing animation off",
            );
            assert!(
                phase.animation_elapsed_ms().is_some(),
                "{order:?} left the animation clock stopped",
            );
            assert!(
                !phase.is_recording(),
                "{order:?} kept drawing the audio visualization",
            );
            assert_eq!(restarts, 1, "{order:?} restarted the animation {restarts}x");
        }
    }

    #[test]
    fn every_interleaving_of_a_cycle_ends_idle() {
        // The mirror property: no ordering may leave the applet animating
        // forever, which is what dropping the `transcribing_started` guard
        // would cause when `transcribing_stopped` arrives first.
        for order in CLOSING_ORDERS {
            let mut phase = recording();
            for event in order {
                phase.apply(event);
            }

            assert!(!phase.is_transcribing(), "{order:?} stayed in transcribing");
            assert!(!phase.is_recording(), "{order:?} stayed in recording");
            assert!(
                phase.animation_elapsed_ms().is_none(),
                "{order:?} left the animation clock running",
            );
        }
    }

    #[test]
    fn every_interleaving_clears_the_visualization() {
        // Whichever event ends the cycle has to retire the audio bars, or the
        // applet redraws the last frame of a finished take.
        for order in CLOSING_ORDERS {
            let mut phase = recording();
            let cleared = order
                .iter()
                .any(|event| phase.apply(*event).visualization_stale);

            assert!(cleared, "{order:?} never marked the visualization stale");
        }
    }

    #[test]
    fn a_late_transcribing_started_is_ignored() {
        let mut phase = recording();
        phase.apply(TranscribingStopped);

        let change = phase.apply(TranscribingStarted);

        assert!(!phase.is_transcribing(), "a closed cycle came back to life");
        assert!(!change.animation_restarted);
        assert!(phase.animation_elapsed_ms().is_none());
    }

    #[test]
    fn a_silent_take_closes_without_animating() {
        // Nothing was spoken, so the daemon skips `transcribing_started`
        // entirely and closes the cycle straight after the mic stops.
        let mut phase = recording();

        assert!(phase.apply(RecordingFlag(false)).visualization_stale);
        assert!(phase.is_transcribing());
        phase.apply(TranscribingStopped);

        assert!(!phase.is_transcribing());
        assert!(phase.animation_elapsed_ms().is_none());
    }

    #[test]
    fn each_cycle_restarts_the_animation() {
        let mut phase = recording();
        for event in CLOSING_ORDERS[0] {
            phase.apply(event);
        }

        phase.apply(RecordingFlag(true));
        let change = phase.apply(TranscribingStarted);

        assert!(
            change.animation_restarted,
            "the second cycle reused the first cycle's animation clock",
        );
    }

    #[test]
    fn a_stray_mic_stop_while_idle_changes_nothing() {
        let mut phase = RecordingPhase::default();

        let change = phase.apply(RecordingFlag(false));

        assert!(!phase.is_recording());
        assert!(!phase.is_transcribing());
        assert_eq!(change, super::PhaseChange::default());
    }
}
