// SPDX-License-Identifier: GPL-3.0-only

//! Secure secret storage using the system keyring (e.g. GNOME Keyring, `KWallet`).
//!
//! Backend secrets are stored with service name "super-stt" under per-backend
//! accounts `backend:<source>:<name>` (written by the settings app, read here
//! at model load). This keeps secrets out of config files entirely.
//!
//! The same keyring also holds the daemon's HTTP session map under
//! `(super-stt, stt-sessions)` — see `daemon::http::internal::auth::tokens::TokenStore` and
//! `get_sessions_blob`/`set_sessions_blob` below.

use log::{debug, info, warn};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const SERVICE_NAME: &str = "super-stt";

/// When `SUPER_STT_KEYRING_MOCK` is set, route session-blob keyring access to
/// an in-memory mock store instead of the system secret service.
///
/// The integration tests spawn the daemon as a subprocess and CI runs
/// headless, where there is no unlocked system keyring — touching the real
/// secret service there blocks on an unlock prompt or fails. This must be
/// called once at daemon startup, before any keyring access, as it sets the
/// process-wide default credential builder.
///
/// Backend-secret access uses `mock_store()` instead and does not go through
/// this builder.
pub fn install_mock_if_requested() {
    if std::env::var_os("SUPER_STT_KEYRING_MOCK").is_some() {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        // Runs before the logger is initialized, so use stderr directly.
        eprintln!("SUPER_STT_KEYRING_MOCK set — using an in-memory keyring (test/CI only)");
    }
}

/// Keyring user (key name) under which the daemon stores all HTTP session
/// metadata as a single JSON blob. See `TokenStore::load_persisted` for
/// the schema. The blob is a single secret rather than one entry per
/// session because the `keyring` crate doesn't expose enumeration —
/// keeping the whole map under one key turns bootstrap into a single
/// `get_password` and mutations into a single `set_password`.
pub const SESSIONS_KEY: &str = "stt-sessions";

/// Keyring account for a backend secret: `backend:<source>:<name>`, where
/// `source` is the backend's repo id (e.g. `github.com/super-stt/openai`).
/// This is the generic per-backend secret store the settings app writes to.
#[must_use]
fn backend_secret_account(source: &str, name: &str) -> String {
    format!("backend:{source}:{name}")
}

/// Process-global in-memory secret store, used in place of the system keyring
/// when running under tests or with `SUPER_STT_KEYRING_MOCK` set.
///
/// The `keyring` crate's mock backend returns a fresh, isolated credential per
/// `Entry::new`, so a set-then-read round-trip across separate calls cannot
/// share state — which makes the real secret behavior untestable headlessly.
/// This map gives backend-secret access stable, process-wide persistence in
/// tests (CI is headless; touching the real secret service hangs on an unlock
/// prompt) while leaving production behavior on the real keyring untouched.
///
/// Under `cfg!(test)`, this store activates for the entire `--lib` test binary
/// (all unit tests in the crate). The `OnceLock` persists process-wide for the
/// lifetime of that binary, so tests sharing it must use unique account keys
/// to avoid collisions.
///
/// Returns `None` in normal (non-test, non-mock) runs, in which case callers
/// fall back to the system keyring.
fn mock_store() -> Option<&'static Mutex<HashMap<String, String>>> {
    static STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    if cfg!(test) || std::env::var_os("SUPER_STT_KEYRING_MOCK").is_some() {
        Some(STORE.get_or_init(|| Mutex::new(HashMap::new())))
    } else {
        None
    }
}

/// Read one keyring account (mock store under test/mock, else the real keyring).
fn kv_get(account: &str) -> Result<Option<String>, String> {
    if let Some(store) = mock_store() {
        return Ok(store
            .lock()
            .expect("mock keyring store poisoned")
            .get(account)
            .cloned());
    }
    let entry = keyring::Entry::new(SERVICE_NAME, account)
        .map_err(|e| format!("Failed to access keyring entry for {account}: {e}"))?;
    match entry.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Failed to read keyring entry {account}: {e}")),
    }
}

/// Write one keyring account (mock store under test/mock, else the real keyring).
fn kv_set(account: &str, value: &str) -> Result<(), String> {
    if let Some(store) = mock_store() {
        store
            .lock()
            .expect("mock keyring store poisoned")
            .insert(account.to_string(), value.to_string());
        return Ok(());
    }
    let entry = keyring::Entry::new(SERVICE_NAME, account)
        .map_err(|e| format!("Failed to access keyring entry for {account}: {e}"))?;
    entry
        .set_password(value)
        .map_err(|e| format!("Failed to store keyring entry {account}: {e}"))
}

/// Delete one keyring account; absent is success (mock store under test/mock,
/// else the real keyring).
fn kv_delete(account: &str) -> Result<(), String> {
    if let Some(store) = mock_store() {
        store
            .lock()
            .expect("mock keyring store poisoned")
            .remove(account);
        return Ok(());
    }
    let entry = keyring::Entry::new(SERVICE_NAME, account)
        .map_err(|e| format!("Failed to access keyring entry for {account}: {e}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Failed to delete keyring entry {account}: {e}")),
    }
}

/// Read a backend secret (e.g. `OPENAI_API_KEY` for a given backend) from the
/// keyring. Returns `Ok(None)` if not set.
///
/// # Errors
///
/// Returns an error if the keyring is unavailable or access fails.
pub fn get_backend_secret(source: &str, name: &str) -> Result<Option<String>, String> {
    kv_get(&backend_secret_account(source, name)).map_err(|e| {
        warn!(
            "Failed to read backend secret {} ({}): {e}",
            name,
            backend_secret_account(source, name)
        );
        e
    })
}

/// Read the daemon's persisted HTTP session blob from the keyring.
///
/// Returns `Ok(None)` if the entry doesn't exist yet (first run / fresh
/// install). The caller is responsible for parsing the JSON.
///
/// # Errors
///
/// Returns an error if the keyring is unavailable or access fails.
pub fn get_sessions_blob() -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(SERVICE_NAME, SESSIONS_KEY)
        .map_err(|e| format!("Failed to access keyring entry for sessions: {e}"))?;

    // Surface the read before it happens: on a *locked* keyring this call
    // blocks on the secret-service unlock prompt, potentially for a long
    // time. Logging first means a stalled startup is explained in the
    // journal ("waiting on keyring unlock") instead of looking like a
    // silent hang.
    info!(
        "Reading the persisted session store from the system keyring; if the keyring \
         is locked, the daemon will wait here until it is unlocked"
    );
    match entry.get_password() {
        Ok(blob) => {
            debug!("Loaded persisted session blob from keyring");
            Ok(Some(blob))
        }
        Err(keyring::Error::NoEntry) => {
            debug!("No persisted session blob in keyring (fresh install)");
            Ok(None)
        }
        Err(e) => {
            warn!("Failed to read session blob from keyring: {e}");
            Err(format!("Failed to read session blob: {e}"))
        }
    }
}

/// Store (or replace) a backend secret in the system keyring.
///
/// # Errors
/// Returns an error if the keyring is unavailable or the write fails.
pub fn set_backend_secret(source: &str, name: &str, value: &str) -> Result<(), String> {
    kv_set(&backend_secret_account(source, name), value)
}

/// Delete a stored backend secret. Missing entries are treated as success.
///
/// # Errors
/// Returns an error if the keyring is unavailable or the delete fails.
pub fn delete_backend_secret(source: &str, name: &str) -> Result<(), String> {
    kv_delete(&backend_secret_account(source, name))
}

/// Whether a backend secret currently has a stored value.
///
/// # Errors
/// Returns an error if the keyring is unavailable or access fails.
pub fn has_backend_secret(source: &str, name: &str) -> Result<bool, String> {
    Ok(kv_get(&backend_secret_account(source, name))?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trips a backend secret through the in-memory store that
    /// `mock_store()` activates under `cfg!(test)`. Uses a unique account so it
    /// cannot collide with other tests sharing the process-global store.
    #[test]
    fn set_then_has_then_delete_roundtrips() {
        let (src, name) = ("github.com/acme/phase-a", "roundtrip_api_key");
        let _ = delete_backend_secret(src, name); // clean slate
        assert!(!has_backend_secret(src, name).unwrap());
        set_backend_secret(src, name, "sk-123").unwrap();
        assert!(has_backend_secret(src, name).unwrap());
        delete_backend_secret(src, name).unwrap();
        assert!(!has_backend_secret(src, name).unwrap());
        delete_backend_secret(src, name).unwrap(); // idempotent
    }
}

/// Write the daemon's HTTP session blob to the keyring. The value is
/// the JSON-serialized full sessions map; passing an empty map clears
/// previously-stored sessions.
///
/// # Errors
///
/// Returns an error if the keyring is unavailable or the write fails.
pub fn set_sessions_blob(value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, SESSIONS_KEY)
        .map_err(|e| format!("Failed to access keyring entry for sessions: {e}"))?;

    entry
        .set_password(value)
        .map_err(|e| format!("Failed to store session blob: {e}"))?;

    debug!("Persisted session blob to keyring");
    Ok(())
}
