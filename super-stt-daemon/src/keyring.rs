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

/// Errors from keyring access. A small typed enum replacing the previous
/// stringly `Result<_, String>`; every variant's `Display` preserves the
/// account context and underlying cause so existing `{e}` interpolations read
/// the same (audit Tier 3 #36).
#[derive(Debug, thiserror::Error)]
pub enum KeyringError {
    /// Opening, reading, writing, or deleting a system-keyring entry failed.
    #[error("keyring access failed for {account}: {source}")]
    Backend {
        account: String,
        #[source]
        source: keyring::Error,
    },
    /// The `spawn_blocking` keyring task panicked or was cancelled.
    #[error("keyring task failed: {0}")]
    Task(String),
}

impl KeyringError {
    fn backend(account: &str, source: keyring::Error) -> Self {
        Self::Backend {
            account: account.to_string(),
            source,
        }
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
    if cfg!(test) || keyring_mock_env_set() {
        Some(STORE.get_or_init(|| Mutex::new(HashMap::new())))
    } else {
        None
    }
}

/// Whether `SUPER_STT_KEYRING_MOCK` requests the in-memory store. Honored only
/// in debug builds (tests / CI); a release binary ignores the env var entirely
/// so a stray or injected variable can't reroute every backend API key and the
/// session store into a non-persistent, unencrypted in-process map (audit 2
/// Tier 1 #6). Mirrors the release gating of the consent-timer bypass
/// (audit Tier 1 #30).
#[cfg(debug_assertions)]
fn keyring_mock_env_set() -> bool {
    std::env::var_os("SUPER_STT_KEYRING_MOCK").is_some()
}

#[cfg(not(debug_assertions))]
fn keyring_mock_env_set() -> bool {
    false
}

/// Read one keyring account (mock store under test/mock, else the real keyring).
fn kv_get(account: &str) -> Result<Option<String>, KeyringError> {
    if let Some(store) = mock_store() {
        return Ok(store
            .lock()
            .expect("mock keyring store poisoned")
            .get(account)
            .cloned());
    }
    let entry = keyring::Entry::new(SERVICE_NAME, account)
        .map_err(|e| KeyringError::backend(account, e))?;
    match entry.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(KeyringError::backend(account, e)),
    }
}

/// Write one keyring account (mock store under test/mock, else the real keyring).
fn kv_set(account: &str, value: &str) -> Result<(), KeyringError> {
    if let Some(store) = mock_store() {
        store
            .lock()
            .expect("mock keyring store poisoned")
            .insert(account.to_string(), value.to_string());
        return Ok(());
    }
    let entry = keyring::Entry::new(SERVICE_NAME, account)
        .map_err(|e| KeyringError::backend(account, e))?;
    entry
        .set_password(value)
        .map_err(|e| KeyringError::backend(account, e))
}

/// Delete one keyring account; absent is success (mock store under test/mock,
/// else the real keyring).
fn kv_delete(account: &str) -> Result<(), KeyringError> {
    if let Some(store) = mock_store() {
        store
            .lock()
            .expect("mock keyring store poisoned")
            .remove(account);
        return Ok(());
    }
    let entry = keyring::Entry::new(SERVICE_NAME, account)
        .map_err(|e| KeyringError::backend(account, e))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(KeyringError::backend(account, e)),
    }
}

/// Read a backend secret (e.g. `OPENAI_API_KEY` for a given backend) from the
/// keyring. Returns `Ok(None)` if not set.
///
/// # Errors
///
/// Returns an error if the keyring is unavailable or access fails.
pub fn get_backend_secret(source: &str, name: &str) -> Result<Option<String>, KeyringError> {
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
pub fn get_sessions_blob() -> Result<Option<String>, KeyringError> {
    // Surface the read before it happens: on a *locked* keyring this call
    // blocks on the secret-service unlock prompt, potentially for a long
    // time. Logging first means a stalled startup is explained in the
    // journal ("waiting on keyring unlock") instead of looking like a
    // silent hang.
    info!(
        "Reading the persisted session store from the system keyring; if the keyring \
         is locked, the daemon will wait here until it is unlocked"
    );
    // The sessions blob is just another keyring account, so route it through
    // `kv_get` — one mock mechanism (the process-global store), not a second
    // one via the credential-builder mock (audit Tier 3 #36).
    match kv_get(SESSIONS_KEY) {
        Ok(Some(blob)) => {
            debug!("Loaded persisted session blob from keyring");
            Ok(Some(blob))
        }
        Ok(None) => {
            debug!("No persisted session blob in keyring (fresh install)");
            Ok(None)
        }
        Err(e) => {
            warn!("Failed to read session blob from keyring: {e}");
            Err(e)
        }
    }
}

/// Store (or replace) a backend secret in the system keyring.
///
/// # Errors
/// Returns an error if the keyring is unavailable or the write fails.
pub fn set_backend_secret(source: &str, name: &str, value: &str) -> Result<(), KeyringError> {
    kv_set(&backend_secret_account(source, name), value)
}

/// Delete a stored backend secret. Missing entries are treated as success.
///
/// # Errors
/// Returns an error if the keyring is unavailable or the delete fails.
pub fn delete_backend_secret(source: &str, name: &str) -> Result<(), KeyringError> {
    kv_delete(&backend_secret_account(source, name))
}

/// Whether a backend secret currently has a stored value.
///
/// # Errors
/// Returns an error if the keyring is unavailable or access fails.
pub fn has_backend_secret(source: &str, name: &str) -> Result<bool, KeyringError> {
    Ok(kv_get(&backend_secret_account(source, name))?.is_some())
}

// Async wrappers for the backend-secret accessors. A keyring lookup goes through
// DBus to the secret service and can stall for seconds on a locked keyring; the
// sync forms above are called from async request handlers, so route them through
// `spawn_blocking` to keep those calls off the async runtime (Tier 3 #4).

/// Async form of [`get_backend_secret`], run on a blocking thread.
///
/// # Errors
/// Returns an error if the keyring is unavailable or access fails.
pub async fn get_backend_secret_async(
    source: String,
    name: String,
) -> Result<Option<String>, KeyringError> {
    tokio::task::spawn_blocking(move || get_backend_secret(&source, &name))
        .await
        .map_err(|e| KeyringError::Task(e.to_string()))?
}

/// Async form of [`set_backend_secret`], run on a blocking thread.
///
/// # Errors
/// Returns an error if the keyring is unavailable or the write fails.
pub async fn set_backend_secret_async(
    source: String,
    name: String,
    value: String,
) -> Result<(), KeyringError> {
    tokio::task::spawn_blocking(move || set_backend_secret(&source, &name, &value))
        .await
        .map_err(|e| KeyringError::Task(e.to_string()))?
}

/// Async form of [`delete_backend_secret`], run on a blocking thread.
///
/// # Errors
/// Returns an error if the keyring is unavailable or the delete fails.
pub async fn delete_backend_secret_async(source: String, name: String) -> Result<(), KeyringError> {
    tokio::task::spawn_blocking(move || delete_backend_secret(&source, &name))
        .await
        .map_err(|e| KeyringError::Task(e.to_string()))?
}

/// Async form of [`has_backend_secret`], run on a blocking thread.
///
/// # Errors
/// Returns an error if the keyring is unavailable or access fails.
pub async fn has_backend_secret_async(source: String, name: String) -> Result<bool, KeyringError> {
    tokio::task::spawn_blocking(move || has_backend_secret(&source, &name))
        .await
        .map_err(|e| KeyringError::Task(e.to_string()))?
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

    /// The sessions blob now routes through `kv_*`, so a set-then-get shares the
    /// process-global mock store and round-trips in-process (audit Tier 3 #36).
    /// Before, the two accessors built isolated `keyring::Entry` credentials
    /// under the builder mock and could not share state.
    #[test]
    fn sessions_blob_roundtrips_through_kv() {
        let blob = r#"{"version":2,"sessions":{}}"#;
        set_sessions_blob(blob).unwrap();
        assert_eq!(get_sessions_blob().unwrap().as_deref(), Some(blob));
    }
}

/// Write the daemon's HTTP session blob to the keyring. The value is
/// the JSON-serialized full sessions map; passing an empty map clears
/// previously-stored sessions.
///
/// # Errors
///
/// Returns an error if the keyring is unavailable or the write fails.
pub fn set_sessions_blob(value: &str) -> Result<(), KeyringError> {
    // Route through `kv_set` for the same reason as `get_sessions_blob`: one
    // mock mechanism, one entry-construction path (audit Tier 3 #36).
    kv_set(SESSIONS_KEY, value)?;
    debug!("Persisted session blob to keyring");
    Ok(())
}
