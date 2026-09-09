// SPDX-License-Identifier: GPL-3.0-only
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub fn with_recording_flag(&self, is_recording: bool) -> Self {
        match (self, is_recording) {
            (_, true) => Self::Recording,
            (Self::Recording | Self::Processing, false) => Self::Processing,
            (Self::Idle, false) => Self::Idle,
        }
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
