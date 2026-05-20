// SPDX-License-Identifier: GPL-3.0-only

//! Application state and domain models.

pub mod models;

// Re-export commonly used types
pub use models::{AudioTheme, ContextPage, DaemonStatus, MenuAction, Page, RecordingStatus};
