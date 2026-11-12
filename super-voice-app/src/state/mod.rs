// SPDX-License-Identifier: GPL-3.0-only

//! Application state and domain models.

pub mod contexts;
pub mod device_offers;
pub mod language;
pub mod model_operations;
pub mod models;
pub mod models_page;
pub mod registry;
pub mod scripts;
pub mod shortcuts;
pub mod speech;
pub mod stage;
pub mod stage_catalog;
pub mod staged_picks;
pub mod update;
pub mod voice;
pub mod voices;

// Re-export commonly used types
pub use models::{
    ActionError, AudioTheme, ContextPage, DaemonStatus, ErrorScope, LanguageResolution, MenuAction,
    ModelsTab, Page, RecordingStatus, Section,
};
pub use speech::{SpeakingStatus, SpeechState};
pub use stage::{BlockStage, Stage};
pub use voice::{VoiceChoice, VoiceResolution, VoiceState};
