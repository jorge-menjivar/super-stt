// SPDX-License-Identifier: GPL-3.0-only

//! Secure API key storage using the system keyring (e.g. GNOME Keyring, `KWallet`).
//!
//! Keys are stored with service name "super-stt" and provider-specific key names
//! (e.g. "openai-api-key"). This keeps secrets out of config files entirely.

use log::{debug, warn};

const SERVICE_NAME: &str = "super-stt";

/// Get an API key for the given provider from the system keyring.
///
/// Returns `Ok(None)` if no key is stored for this provider.
///
/// # Errors
///
/// Returns an error if the keyring is unavailable or access fails.
pub fn get_api_key(provider: &str) -> Result<Option<String>, String> {
    let key_name = format!("{provider}-api-key");
    let entry = keyring::Entry::new(SERVICE_NAME, &key_name)
        .map_err(|e| format!("Failed to access keyring entry for {provider}: {e}"))?;

    match entry.get_password() {
        Ok(password) => {
            debug!("Retrieved API key for {provider} from keyring");
            Ok(Some(password))
        }
        Err(keyring::Error::NoEntry) => {
            debug!("No API key found for {provider} in keyring");
            Ok(None)
        }
        Err(e) => {
            warn!("Failed to read API key for {provider} from keyring: {e}");
            Err(format!("Failed to read API key for {provider}: {e}"))
        }
    }
}

/// Store an API key for the given provider in the system keyring.
///
/// # Errors
///
/// Returns an error if the keyring is unavailable or the write fails.
pub fn set_api_key(provider: &str, key: &str) -> Result<(), String> {
    let key_name = format!("{provider}-api-key");
    let entry = keyring::Entry::new(SERVICE_NAME, &key_name)
        .map_err(|e| format!("Failed to access keyring entry for {provider}: {e}"))?;

    entry
        .set_password(key)
        .map_err(|e| format!("Failed to store API key for {provider}: {e}"))?;

    debug!("Stored API key for {provider} in keyring");
    Ok(())
}

/// Delete an API key for the given provider from the system keyring.
///
/// Returns `Ok(())` even if no key was stored (idempotent).
///
/// # Errors
///
/// Returns an error if the keyring is unavailable or deletion fails for reasons
/// other than the key not existing.
pub fn delete_api_key(provider: &str) -> Result<(), String> {
    let key_name = format!("{provider}-api-key");
    let entry = keyring::Entry::new(SERVICE_NAME, &key_name)
        .map_err(|e| format!("Failed to access keyring entry for {provider}: {e}"))?;

    match entry.delete_credential() {
        Ok(()) => {
            debug!("Deleted API key for {provider} from keyring");
            Ok(())
        }
        Err(keyring::Error::NoEntry) => {
            debug!("No API key to delete for {provider}");
            Ok(())
        }
        Err(e) => {
            warn!("Failed to delete API key for {provider} from keyring: {e}");
            Err(format!("Failed to delete API key for {provider}: {e}"))
        }
    }
}

/// Check whether an API key exists for the given provider without reading it.
///
/// # Errors
///
/// Returns an error if the keyring is unavailable.
pub fn has_api_key(provider: &str) -> Result<bool, String> {
    get_api_key(provider).map(|key| key.is_some())
}
