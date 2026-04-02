// SPDX-License-Identifier: GPL-3.0-only
use cosmic::{iced::window, widget::segmented_button::Entity};

use crate::models::{
    state::{IsOpen, RecordingState},
    theme::{VisualizationColor, VisualizationTheme},
};

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    CloseRequested(window::Id),
    DaemonConnected,
    DaemonConfigReceived(serde_json::Value),
    DaemonError(String),
    RecordingStateChanged(RecordingState),
    AudioLevelUpdate {
        level: f32,
        is_speech: bool,
    },
    UdpData(Vec<u8>),
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
    SetAppletWidth(u32),
    SetShowIcon(bool),
    SetIconAlignmentEntity(Entity),
    SetShowVisualizations(bool),
    SetVisualizationColor(VisualizationColor, bool), // Color and is_dark flag
    SetColorThemeEntity(Entity),                     // Theme selector for color configuration
}
