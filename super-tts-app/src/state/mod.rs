// SPDX-License-Identifier: GPL-3.0-only

//! Application state and domain models.

pub mod language;
pub mod models;
pub mod models_page;
pub mod registry;
pub mod scripts;
pub mod update;
pub mod voices;

// Re-export commonly used types
pub use models::{
    ActionError, AudioTheme, ContextPage, DaemonStatus, ErrorScope, LanguageResolution, MenuAction,
    ModelsTab, Page, SpeakingStatus, VoiceChoice, VoiceResolution, VoiceState,
};
