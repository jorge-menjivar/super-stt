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

/// Audio level data from UDP packets
#[derive(Debug)]
pub struct AudioLevelData {
    pub level: f32,
    pub is_speech: bool,
}
