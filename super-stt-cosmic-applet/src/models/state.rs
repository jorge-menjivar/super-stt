// SPDX-License-Identifier: GPL-3.0-only
#[derive(Debug, Clone)]
pub enum RecordingState {
    Idle,
    Recording,
    Processing,
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
