// SPDX-License-Identifier: GPL-3.0-only

//! Application state and domain models.

pub mod language;
pub mod models;
pub mod models_page;
pub mod registry;

// Re-export commonly used types
pub use models::{
    ActionError, AudioTheme, ContextPage, DaemonStatus, ErrorScope, LanguageResolution, MenuAction,
    ModelsTab, Page, RecordingStatus,
};
