// SPDX-License-Identifier: GPL-3.0-only
use cosmic::{iced::window, widget::segmented_button::Entity};

use crate::models::{
    state::{IsOpen, RecordingState},
    theme::{VisualizationColor, VisualizationTheme, WorkingAnimationTheme},
};

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    CloseRequested(window::Id),
    DaemonConnected,
    DaemonError(String),
    RecordingStateChanged(RecordingState),
    AudioLevelUpdate {
        level: f32,
        is_speech: bool,
    },
    /// `recording_state` event from the daemon's `/events` SSE stream.
    WidgetRecordingState(bool),
    /// `frequency_bands` event — pre-computed visualization bands.
    WidgetFrequencyBands {
        bands: Vec<f32>,
        sample_rate: f32,
        total_energy: f32,
    },
    /// `transcribing_started` event — the daemon began decoding captured audio.
    WidgetTranscribingStarted,
    /// `transcribing_stopped` event — decode + typing finished; cycle is idle.
    WidgetTranscribingStopped,
    /// `revoked` event — daemon dropped the session. Reason is the
    /// value of the SSE event's `reason` field (e.g. `"exe_changed"`).
    WidgetRevoked(String),
    /// Any other widget event we don't care about (or a parse failure).
    /// Carries the raw daemon event name for logging only.
    WidgetOtherEvent(String),
    /// Subscription failed before/after handshake; UI returns to error
    /// retry path. String is a daemon-supplied message.
    WidgetSubscriptionError(String),
    /// User denied the consent prompt. Subscription terminated and
    /// won't auto-retry — the user must click the popup's "Retry
    /// authorization" button after restarting the daemon.
    WidgetBlocked(String),
    /// Triggered by the popup's Retry button. Clears the cached
    /// session token and restarts the `/events` subscription so the
    /// daemon spawns a fresh consent prompt.
    RetryAuthorization,
    RetryConnection,
    ScheduleRetry,
    PingTimeout,
    PingResponse {
        message: String,
        connection_active: bool,
    },
    OpenGitHub,
    LaunchApp,
    RevealerToggle(IsOpen),
    SetVisualizationTheme(VisualizationTheme),
    /// Pick the working/transcribing animation style.
    SetWorkingAnimation(WorkingAnimationTheme),
    SetAppletWidth(u32),
    SetShowIcon(bool),
    SetIconAlignmentEntity(Entity),
    SetShowVisualizations(bool),
    SetVisualizationColor(VisualizationColor, bool), // Color and is_dark flag
    SetColorThemeEntity(Entity),                     // Theme selector for color configuration
    /// Animation frame tick while transcribing (drives the working animation).
    WorkingAnimationTick,
}
