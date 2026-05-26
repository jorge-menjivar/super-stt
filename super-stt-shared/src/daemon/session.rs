// SPDX-License-Identifier: GPL-3.0-only
//! Session-token cache for HTTP-protocol clients.
//!
//! Two layers of caching, in order of priority:
//!
//! 1. **In-memory cache** (this module's `TOKEN_CACHE`). Hot path —
//!    set on the first successful `obtain` and reused for the rest of
//!    the process's lifetime. No keyring access on cache hit, which
//!    matters a lot when a long-lived widget reconnects in a tight
//!    loop while the daemon is down.
//! 2. **System keyring** (libsecret/KWallet). Cold-start persistence
//!    — read once on first `obtain` to recover a token from a previous
//!    process run. Best-effort write whenever a fresh token is minted.
//!
//! Each app gets its own keyring "user" (= [`AppId`] string) so they
//! don't overwrite each other's tokens. The storage value is just the
//! bearer string; scope and expiry live server-side and the daemon
//! returns `invalid_session` if the client presents a stale token.

use crate::daemon::http_client;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use tokio::sync::Mutex as AsyncMutex;

const KEYRING_SERVICE: &str = "super-stt-session";

/// Per-app keyring "user" identifier. Pick a stable string that uniquely
/// identifies your app (e.g. `"super-stt-cli"`, `"super-stt-app"`).
#[derive(Clone, Copy, Debug)]
pub struct AppId(pub &'static str);

type ObtainLock = Arc<AsyncMutex<()>>;
type ObtainLockMap = StdMutex<HashMap<&'static str, ObtainLock>>;

/// Per-`AppId` async mutex registry. Ensures at most one
/// `auth_request` flight is in progress per app at any time so parallel
/// callers (e.g. the settings app's batch of 6 startup GETs) can't each
/// independently spawn a consent popup. Held across the `auth_request`
/// await; tokens cached in the keyring after the first caller wins, so
/// subsequent callers double-check `load()` and skip the network entirely.
static OBTAIN_LOCKS: LazyLock<ObtainLockMap> = LazyLock::new(|| StdMutex::new(HashMap::new()));

fn lock_for(app_id: AppId) -> ObtainLock {
    let mut map = OBTAIN_LOCKS.lock().unwrap();
    map.entry(app_id.0)
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}

/// In-process token cache. Populated on every successful `obtain`,
/// consulted before any keyring access. This is what lets a tight
/// reconnect loop (e.g. while the daemon is down) avoid hammering the
/// keyring. Cleared by `forget`.
static TOKEN_CACHE: LazyLock<StdMutex<HashMap<&'static str, String>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

fn cache_get(app_id: AppId) -> Option<String> {
    TOKEN_CACHE.lock().unwrap().get(app_id.0).cloned()
}

fn cache_set(app_id: AppId, token: String) {
    TOKEN_CACHE.lock().unwrap().insert(app_id.0, token);
}

fn cache_clear(app_id: AppId) {
    TOKEN_CACHE.lock().unwrap().remove(app_id.0);
}

/// Read the cached token for `app_id`, or None if no token is stored.
#[must_use]
pub fn load(app_id: AppId) -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, app_id.0).ok()?;
    entry.get_password().ok()
}

/// Persist a token for `app_id` to both the in-memory cache and the
/// system keyring. Replaces any previous value. The in-memory side
/// always succeeds; the keyring write is best-effort and its failure
/// is reported via the return value (callers in this module ignore it
/// because the in-memory cache is the source of truth at runtime).
///
/// # Errors
/// Returns an error if the keyring is unavailable or the write fails.
pub fn save(app_id: AppId, token: &str) -> Result<(), String> {
    cache_set(app_id, token.to_string());
    let entry = keyring::Entry::new(KEYRING_SERVICE, app_id.0)
        .map_err(|e| format!("keyring access failed: {e}"))?;
    entry
        .set_password(token)
        .map_err(|e| format!("keyring write failed: {e}"))?;
    Ok(())
}

/// Forget the cached token for `app_id` (both in-memory and the
/// keyring). Idempotent — succeeds even if nothing was stored.
///
/// # Errors
/// Returns an error if the keyring is unavailable.
pub fn forget(app_id: AppId) -> Result<(), String> {
    cache_clear(app_id);
    let entry = keyring::Entry::new(KEYRING_SERVICE, app_id.0)
        .map_err(|e| format!("keyring access failed: {e}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete failed: {e}")),
    }
}

/// Get a usable session token for `app_id`. Cascades through three
/// layers, in order:
///
/// 1. **In-memory cache.** Hot path; no I/O, no keyring access.
/// 2. **System keyring.** Cold path on first call after a process
///    start. Populates the in-memory cache on success.
/// 3. **`auth_request`.** Triggers the libcosmic consent popup. Stores
///    the resulting token in both the cache and the keyring (the
///    keyring write is best-effort — if it fails the in-memory cache
///    still keeps the token alive for the rest of the process).
///
/// Concurrency-safe: parallel callers for the same `app_id` are
/// serialized through a per-`AppId` async mutex (double-checked
/// locking against the cache + keyring), so at most one consent popup
/// is ever spawned even when the settings app fires its startup batch
/// of six settings GETs in parallel.
///
/// # Errors
/// Returns an error if `auth_request` fails (user denied, popup
/// dismissed, daemon unreachable, etc.). Keyring write failures are
/// silently absorbed (the token remains usable for this process).
pub async fn obtain(
    socket_path: PathBuf,
    app_id: AppId,
    app_name: &str,
    scope: &str,
) -> http_client::HttpResult<String> {
    // 1. In-memory cache hit — no keyring access, no I/O.
    if let Some(t) = cache_get(app_id) {
        return Ok(t);
    }

    // 2. Keyring read (one-time per process per AppId, populates the
    //    in-memory cache for subsequent calls).
    if let Some(t) = load(app_id) {
        cache_set(app_id, t.clone());
        return Ok(t);
    }

    // 3. Slow path: serialize concurrent first-time obtains so we
    //    don't fire N parallel consent popups.
    let app_lock = lock_for(app_id);
    let _guard = app_lock.lock().await;

    // Re-check after acquiring the lock: another task may have
    // already minted a token while we were waiting.
    if let Some(t) = cache_get(app_id) {
        return Ok(t);
    }
    if let Some(t) = load(app_id) {
        cache_set(app_id, t.clone());
        return Ok(t);
    }

    let auth = http_client::auth_request(socket_path, app_name, scope).await?;
    // `save` updates both in-memory cache and keyring; we ignore the
    // keyring half's error so a locked / denied keyring doesn't break
    // the working session.
    let _ = save(app_id, &auth.session_token);
    Ok(auth.session_token)
}

/// Run `op` with the cached or freshly-minted token. On
/// [`HttpError::InvalidSession`] from the daemon, drops the cached
/// token and retries `op` once with a fresh consent flow.
///
/// `op` returns `Result<T, String>` so the iced UI plumbing can
/// shovel error strings straight into toasts. Internally the retry
/// decision parses the wire-deterministic `HttpError::Display`
/// prefix — `invalid_session (<reason>)` is produced by exactly one
/// site in `http_client.rs` ([`http_client::HttpError::InvalidSession`]),
/// so this substring inspection is checking content the shared crate
/// just produced rather than arbitrary error text.
///
/// # Errors
/// Returns the underlying error if `op` fails for any non-auth reason
/// or if re-auth fails.
pub async fn with_token<F, Fut, T>(
    socket_path: PathBuf,
    app_id: AppId,
    app_name: &str,
    scope: &str,
    op: F,
) -> Result<T, String>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let token = obtain(socket_path.clone(), app_id, app_name, scope)
        .await
        .map_err(|e| e.to_string())?;
    match op(token).await {
        Ok(v) => Ok(v),
        Err(e) if is_wire_invalid_session(&e) => {
            // Token rejected — drop cache, re-auth, retry once.
            let _ = forget(app_id);
            let token = obtain(socket_path, app_id, app_name, scope)
                .await
                .map_err(|e| e.to_string())?;
            op(token).await
        }
        Err(e) => Err(e),
    }
}

/// True if `s` matches the deterministic prefix
/// `HttpError::InvalidSession::Display` emits. Centralized so the
/// retry-on-401 logic isn't matching arbitrary error text — only
/// strings the shared `http_client` produced.
fn is_wire_invalid_session(s: &str) -> bool {
    s.starts_with("invalid_session (")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that once a token is in the in-memory cache, `obtain`
    /// returns it without touching the keyring or the network. We
    /// pass a bogus socket path that would fail if `obtain` fell
    /// through to `auth_request`.
    #[tokio::test]
    async fn obtain_returns_from_cache_without_network() {
        let app_id = AppId("test-cache-hit");
        // Manually pre-populate the cache.
        cache_set(app_id, "TOK-from-cache".to_string());

        let bogus_socket = PathBuf::from("/nonexistent/super-stt/socket");
        let result = obtain(bogus_socket, app_id, "Test", "client").await;

        // Cleanup before asserting (in case the assert panics, the
        // global cache stays clean for sibling tests).
        cache_clear(app_id);

        assert_eq!(result.expect("cache hit should succeed"), "TOK-from-cache");
    }

    /// Verify the in-memory cache primitives round-trip cleanly. We
    /// don't exercise `forget`/`save` directly here because both
    /// touch the real system keyring, and a unit test under a locked
    /// keyring would hang on the unlock prompt. The `forget` and
    /// `save` functions invoke `cache_clear` and `cache_set`
    /// respectively as their first action, so a working cache layer
    /// is the necessary-and-sufficient ingredient.
    #[test]
    fn cache_set_get_clear_round_trip() {
        let app_id = AppId("test-cache-roundtrip");
        cache_clear(app_id);
        assert_eq!(cache_get(app_id), None, "fresh slot must be empty");

        cache_set(app_id, "TOK-a".to_string());
        assert_eq!(cache_get(app_id), Some("TOK-a".to_string()));

        // Replace.
        cache_set(app_id, "TOK-b".to_string());
        assert_eq!(cache_get(app_id), Some("TOK-b".to_string()));

        cache_clear(app_id);
        assert_eq!(cache_get(app_id), None, "clear must drop the entry");
    }
}
