// SPDX-License-Identifier: GPL-3.0-only

//! Secure API key storage using the system keyring.

const SERVICE_NAME: &str = "super-stt";

pub fn set_api_key(provider: &str, key: &str) -> Result<(), String> {
    let key_name = format!("{provider}-api-key");
    let entry = keyring::Entry::new(SERVICE_NAME, &key_name)
        .map_err(|e| format!("Failed to access keyring: {e}"))?;
    entry
        .set_password(key)
        .map_err(|e| format!("Failed to store API key: {e}"))
}

pub fn delete_api_key(provider: &str) -> Result<(), String> {
    let key_name = format!("{provider}-api-key");
    let entry = keyring::Entry::new(SERVICE_NAME, &key_name)
        .map_err(|e| format!("Failed to access keyring: {e}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Failed to delete API key: {e}")),
    }
}

pub fn has_api_key(provider: &str) -> Result<bool, String> {
    let key_name = format!("{provider}-api-key");
    let entry = keyring::Entry::new(SERVICE_NAME, &key_name)
        .map_err(|e| format!("Failed to access keyring: {e}"))?;
    match entry.get_password() {
        Ok(_) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(format!("Failed to check keyring: {e}")),
    }
}
