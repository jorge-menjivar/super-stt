// SPDX-License-Identifier: GPL-3.0-only

//! Data models and types for the Super STT application.

// Re-export AudioTheme from shared crate
pub use super_stt_shared::models::theme::AudioTheme;

/// Daemon connection status
#[derive(Debug, Clone, Default, PartialEq)]
pub enum DaemonStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error(String),
    /// User denied the settings-scope consent prompt (either just now
    /// or via the daemon's sticky deny cache). All settings-scope
    /// operations will fail until the user explicitly retries
    /// authorization — usually after `systemctl --user restart
    /// super-stt`. The auto-retry loop is suppressed in this state to
    /// avoid spamming the daemon's deny cache.
    Blocked(String),
}

/// Recording status
#[derive(Debug, Clone, Default, PartialEq)]
pub enum RecordingStatus {
    #[default]
    Idle,
    Recording,
}

/// The page to display in the application
#[derive(Debug, Clone)]
pub enum Page {
    Connection,
    Customization,
    Recording,
    InputSimulation,
    Models,
    OnlineModels,
}

/// The context page to display in the context drawer
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    ModelSelection,
}

/// Menu actions
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    About,
}
