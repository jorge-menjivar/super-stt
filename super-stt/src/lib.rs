// SPDX-License-Identifier: GPL-3.0-only
pub mod audio;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod download_progress;
pub mod input;
pub mod keyring;
pub mod output;
pub mod services;
pub mod stt_models;

// Re-export the main run function
pub use daemon_main::run;

mod daemon_main;
