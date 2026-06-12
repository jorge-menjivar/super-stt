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

const SERVICE_NAME: &str = "super-stt";

/// When `SUPER_STT_KEYRING_MOCK` is set, route all keyring access to an
/// in-memory mock store instead of the system secret service.
///
/// The integration tests spawn the daemon as a subprocess and CI runs
/// headless, where there is no unlocked system keyring — touching the real
/// secret service there blocks on an unlock prompt or fails. This must be
/// called once at daemon startup, before any keyring access, as it sets the
/// process-wide default credential builder.
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

/// Read a backend secret (e.g. `OPENAI_API_KEY` for a given backend) from the
/// keyring. Returns `Ok(None)` if not set.
///
/// # Errors
///
/// Returns an error if the keyring is unavailable or access fails.
pub fn get_backend_secret(source: &str, name: &str) -> Result<Option<String>, String> {
    let account = backend_secret_account(source, name);
    let entry = keyring::Entry::new(SERVICE_NAME, &account)
        .map_err(|e| format!("Failed to access keyring entry for {account}: {e}"))?;
    match entry.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => {
            warn!("Failed to read backend secret {account}: {e}");
            Err(format!("Failed to read backend secret {name}: {e}"))
        }
    }
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
