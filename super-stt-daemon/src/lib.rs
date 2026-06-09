// SPDX-License-Identifier: GPL-3.0-only
pub mod audio;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod download_progress;
pub mod input;
pub mod keyring;
pub mod output;
pub mod registry;
pub mod services;
pub mod stt_models;

// Re-export the main run function
pub use daemon_main::run;

mod daemon_main;
mod num_cast;

/// Install the ring crypto provider for rustls.
/// Safe to call multiple times — returns Ok on first call, Err on subsequent (which we ignore).
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
