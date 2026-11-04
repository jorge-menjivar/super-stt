// SPDX-License-Identifier: GPL-3.0-only

//! Super TTS's secrets in the system keyring: each backend's API credentials
//! and the daemon's session tokens, under the `super-tts` service. The store
//! itself is `super_engine_daemon::keyring`.
//!
//! The daemon's own unit tests use the in-memory store instead, so they never
//! touch the developer's secrets.

use super_engine_daemon::keyring::Keyring;
pub use super_engine_daemon::keyring::KeyringError;
use super_tts_shared::SUPER_TTS;

/// Super TTS's keyring: the system one, or the in-memory store under test.
#[must_use]
pub fn keyring() -> Keyring {
    if cfg!(test) {
        Keyring::in_memory(&SUPER_TTS)
    } else {
        Keyring::system(&SUPER_TTS)
    }
}

/// Read a backend secret on a blocking thread. `Ok(None)` when it is not set.
///
/// # Errors
/// Returns an error if the keyring is unavailable or access fails.
pub async fn get_backend_secret_async(
    source: String,
    name: String,
) -> Result<Option<String>, KeyringError> {
    keyring().get_backend_secret_async(source, name).await
}

/// Store (or replace) a backend secret on a blocking thread.
///
/// # Errors
/// Returns an error if the keyring is unavailable or the write fails.
pub async fn set_backend_secret_async(
    source: String,
    name: String,
    value: String,
) -> Result<(), KeyringError> {
    keyring()
        .set_backend_secret_async(source, name, value)
        .await
}

/// Delete a stored backend secret on a blocking thread. Missing entries are
/// treated as success.
///
/// # Errors
/// Returns an error if the keyring is unavailable or the delete fails.
pub async fn delete_backend_secret_async(source: String, name: String) -> Result<(), KeyringError> {
    keyring().delete_backend_secret_async(source, name).await
}

/// Whether a backend secret currently has a stored value, on a blocking
/// thread.
///
/// # Errors
/// Returns an error if the keyring is unavailable or access fails.
pub async fn has_backend_secret_async(source: String, name: String) -> Result<bool, KeyringError> {
    keyring().has_backend_secret_async(source, name).await
}

#[cfg(test)]
mod tests {
    /// The sessions account is the one Super TTS has always used. A change
    /// here strands every installed daemon's sessions under an account it no
    /// longer reads, and every client faces a fresh consent popup.
    #[test]
    fn the_sessions_account_is_the_one_super_tts_shipped() {
        assert_eq!(super::keyring().sessions_account(), "tts-sessions");
    }
}
