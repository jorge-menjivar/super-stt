// SPDX-License-Identifier: GPL-3.0-only
pub mod audio;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod download_progress;
pub mod download_stream;
pub mod input;
pub mod keyring;
pub mod output;
pub mod registry;
pub mod resource_management;
pub mod services;
pub mod stt_models;

// Re-export the main run function
pub use daemon_main::run;

mod daemon_main;
mod num_cast;

/// Re-export the shared rustls installer from `super-stt-forge`, so
/// `main` and the tests install the provider through one implementation that
/// lives beside the reqwest client factory.
pub use super_stt_forge::install_crypto_provider;
