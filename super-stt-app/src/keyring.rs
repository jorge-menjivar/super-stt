// SPDX-License-Identifier: GPL-3.0-only

//! Secure API key storage using the system keyring.

const SERVICE_NAME: &str = "super-stt";

/// Account string for a backend secret in the system keyring.
///
/// The daemon reads the same `(service, account)` pair at model-load
/// time, so this format must stay in lockstep with the daemon side:
/// service `"super-stt"`, account `"backend:{source}:{name}"`.
fn backend_account(source: &str, name: &str) -> String {
    format!("backend:{source}:{name}")
}

/// Store a backend-declared secret (e.g. an API key) in the system keyring.
pub fn set_backend_secret(source: &str, name: &str, value: &str) -> Result<(), String> {
    let account = backend_account(source, name);
    let entry = keyring::Entry::new(SERVICE_NAME, &account)
        .map_err(|e| format!("Failed to access keyring: {e}"))?;
    entry
        .set_password(value)
        .map_err(|e| format!("Failed to store secret: {e}"))
}

/// Delete a stored backend secret. Missing entries are treated as success.
pub fn delete_backend_secret(source: &str, name: &str) -> Result<(), String> {
    let account = backend_account(source, name);
    let entry = keyring::Entry::new(SERVICE_NAME, &account)
        .map_err(|e| format!("Failed to access keyring: {e}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Failed to delete secret: {e}")),
    }
}

/// Check whether a backend secret is currently stored.
pub fn has_backend_secret(source: &str, name: &str) -> Result<bool, String> {
    let account = backend_account(source, name);
    let entry = keyring::Entry::new(SERVICE_NAME, &account)
        .map_err(|e| format!("Failed to access keyring: {e}"))?;
    match entry.get_password() {
        Ok(_) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(format!("Failed to check keyring: {e}")),
    }
}
