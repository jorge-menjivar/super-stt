// SPDX-License-Identifier: GPL-3.0-only

//! Application state and domain models.

pub mod device_offers;
pub mod language;
pub mod model_operations;
pub mod models;
pub mod models_page;
pub mod registry;
pub mod stage_catalog;
pub mod staged_picks;
pub mod update;

// Re-export commonly used types
pub use models::{
    ActionError, AudioTheme, ContextPage, DaemonStatus, ErrorScope, LanguageResolution, MenuAction,
    ModelsTab, Page, RecordingStatus,
};
