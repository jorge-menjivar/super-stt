// SPDX-License-Identifier: GPL-3.0-only
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use log::{error, info, warn};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

/// Schema version for the persisted sessions blob. Bump on any breaking
/// change to `TokenMeta`'s on-disk shape so an older daemon can refuse
/// to load a newer file rather than misinterpret fields.
pub(crate) const SESSIONS_SCHEMA_VERSION: u32 = 2;

/// After a keyring write fails, suppress further attempts for this
/// long. A locked keyring would otherwise re-prompt the user every
/// time a session is minted, expired, or revoked. Cleared by the next
/// successful write.
pub(crate) const KEYRING_FAILURE_COOLDOWN: Duration = Duration::from_mins(5);

/// Tracks the most recent keyring-write failure timestamp so
/// `flush_locked` can short-circuit during the cooldown window.
pub(crate) static KEYRING_LAST_FAILURE: std::sync::LazyLock<Mutex<Option<std::time::Instant>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

pub(crate) fn keyring_writes_in_cooldown() -> bool {
    KEYRING_LAST_FAILURE
        .lock()
        .unwrap()
        .is_some_and(|t| t.elapsed() < KEYRING_FAILURE_COOLDOWN)
}

pub(crate) fn mark_keyring_failure() {
    *KEYRING_LAST_FAILURE.lock().unwrap() = Some(std::time::Instant::now());
}

pub(crate) fn clear_keyring_failure_flag() {
    *KEYRING_LAST_FAILURE.lock().unwrap() = None;
}

/// When set to `1`, the daemon starts even if the system keyring is
/// unavailable, falling back to an ephemeral in-memory session store that
/// does not survive a restart. Intended for headless / CI hosts that have
/// no secret service. Without it, an unavailable keyring is fatal: the
/// daemon refuses to start rather than silently run without session
/// persistence.
pub(crate) const ALLOW_NO_KEYRING_ENV: &str = "SUPER_STT_ALLOW_NO_KEYRING";

fn allow_no_keyring() -> bool {
    std::env::var(ALLOW_NO_KEYRING_ENV).is_ok_and(|v| v == "1")
}

/// A message to the single-writer persist task.
// Both fields are only read in `persist_loop`, which is `#[cfg(not(test))]`; the
// constructors compile in test builds too, so the fields read as dead code under
// `cargo test`. They're live in production — suppress the test-only lint.
#[cfg_attr(test, allow(dead_code))]
enum PersistMsg {
    /// Persist this sessions snapshot (coalesced with any newer ones).
    Snapshot(HashMap<String, TokenMeta>),
    /// Drain the backlog, write the latest snapshot, then signal completion via
    /// the oneshot. The shutdown path uses this so a token minted or revoked in
    /// the final moments is durably written before `process::exit` skips the
    /// task (audit 2 Tier 1 #5).
    Flush(tokio::sync::oneshot::Sender<()>),
}

/// Process-global handle to the live persist task's channel. Registered when the
/// single production `TokenStore` is created ([`SessionPersister::spawn`], only
/// in non-test builds). [`flush_persisted_sessions`] uses it to drain queued
/// writes on shutdown. `None` in `cargo test` builds (no task is spawned) and
/// until the production store exists.
static PERSIST_FLUSH: std::sync::OnceLock<mpsc::UnboundedSender<PersistMsg>> =
    std::sync::OnceLock::new();

/// Drain any queued session snapshots to the keyring before the daemon exits.
///
/// The persist task is fire-and-forget; the shutdown path's `process::exit`
/// would otherwise skip it, losing a token minted or revoked in the sub-second
/// window before shutdown (audit 2 Tier 1 #5). Bounded by `timeout` so a stalled
/// keyring (a D-Bus unlock prompt) can't hang shutdown indefinitely.
pub(crate) async fn flush_persisted_sessions(timeout: Duration) {
    let Some(tx) = PERSIST_FLUSH.get() else {
        return; // no production persist task (test build, or store never created)
    };
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    if tx.send(PersistMsg::Flush(ack_tx)).is_err() {
        return; // persist task already ended
    }
    match tokio::time::timeout(timeout, ack_rx).await {
        Ok(Ok(())) => info!("Session store flushed to keyring on shutdown"),
        Ok(Err(_)) => {} // ack sender dropped: task ended without acking
        Err(_) => warn!(
            "Session-store flush timed out after {}s on shutdown; queued writes may be lost",
            timeout.as_secs()
        ),
    }
}

/// Single-writer handle for persisting the sessions map to the keyring.
///
/// `mint`/`revoke`/`validate` are sync fns called from async request handlers;
/// writing the whole map to the keyring inline blocks a runtime worker on `DBus`
/// I/O, and two concurrent mutations dropping their `Mutex` guard before writing
/// would race the on-disk order. Funnelling every snapshot through one task over
/// this channel keeps the blocking write off the request path *and* serializes
/// the writes so the last submitted snapshot is the last one written (audit
/// Tier 3 #35). Callers submit under the in-memory lock so channel order matches
/// lock order (audit 2 Tier 1 #4).
#[derive(Clone)]
pub(crate) struct SessionPersister {
    tx: mpsc::UnboundedSender<PersistMsg>,
}

impl SessionPersister {
    /// Spawn the single-writer persist task and return a submit handle. In
    /// `cargo test` builds no task is spawned (unit tests must not touch the
    /// developer's keyring); submissions become no-ops as the receiver is
    /// dropped.
    fn spawn() -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<PersistMsg>();
        #[cfg(not(test))]
        {
            tokio::spawn(persist_loop(rx));
            // Register the first (production) persister so the shutdown path can
            // drain it. Production creates exactly one `TokenStore` (AppState's);
            // any later store keeps its own channel but won't be flushed.
            let _ = PERSIST_FLUSH.set(tx.clone());
        }
        #[cfg(test)]
        drop(rx);
        Self { tx }
    }

    /// Submit the latest sessions snapshot for persistence. Best-effort: a
    /// closed channel (test builds) just means the write is skipped — the
    /// in-memory map stays authoritative.
    fn submit(&self, snapshot: HashMap<String, TokenMeta>) {
        let _ = self.tx.send(PersistMsg::Snapshot(snapshot));
    }
}

/// The persist task: receive messages, coalesce a burst of snapshots down to the
/// newest (older snapshots are strict subsets of the state the newest
/// represents), write it, then ack any flush request in the same batch — so a
/// shutdown flush is acked only after the latest state is on disk.
#[cfg(not(test))]
async fn persist_loop(mut rx: mpsc::UnboundedReceiver<PersistMsg>) {
    while let Some(first) = rx.recv().await {
        let mut latest: Option<HashMap<String, TokenMeta>> = None;
        let mut flush_ack: Option<tokio::sync::oneshot::Sender<()>> = None;
        let mut msg = Some(first);
        while let Some(m) = msg {
            match m {
                PersistMsg::Snapshot(s) => latest = Some(s),
                PersistMsg::Flush(ack) => flush_ack = Some(ack),
            }
            msg = rx.try_recv().ok();
        }
        if let Some(snapshot) = latest {
            persist_snapshot(snapshot).await;
        }
        if let Some(ack) = flush_ack {
            let _ = ack.send(());
        }
    }
}

/// Serialize one snapshot and write it to the keyring on a blocking thread.
/// Runs inside the single persist task, so writes are already serialized.
///
/// **Failure suppression.** A locked or denied keyring fails every write;
/// without a cooldown a busy mint/revoke loop would re-prompt the user every
/// few seconds. After one failure we suppress further writes for
/// [`KEYRING_FAILURE_COOLDOWN`]; the in-memory map stays authoritative and a
/// later success clears the flag.
#[cfg(not(test))]
async fn persist_snapshot(snapshot: HashMap<String, TokenMeta>) {
    if keyring_writes_in_cooldown() {
        return;
    }
    let payload = SessionsFile {
        version: SESSIONS_SCHEMA_VERSION,
        sessions: snapshot,
    };
    let json = match serde_json::to_string(&payload) {
        Ok(j) => j,
        Err(e) => {
            warn!("Failed to serialize sessions blob: {e}");
            return;
        }
    };
    // The keyring write is blocking DBus I/O; keep it off the task's async
    // executor. The task awaits it, so writes stay strictly ordered.
    match tokio::task::spawn_blocking(move || crate::keyring::set_sessions_blob(&json)).await {
        Ok(Ok(())) => clear_keyring_failure_flag(),
        Ok(Err(e)) => {
            warn!(
                "Failed to persist sessions blob: {e}; suppressing further keyring writes for {}s",
                KEYRING_FAILURE_COOLDOWN.as_secs()
            );
            mark_keyring_failure();
        }
        Err(e) => warn!("Session persist task panicked: {e}"),
    }
}

/// Persistent store of issued session tokens. The in-memory `HashMap`
/// is the hot lookup path; every mutation also writes the whole map
/// back to the system keyring under `(super-stt, stt-sessions)` so a
/// daemon restart re-hydrates the same set of valid tokens.
#[derive(Clone)]
pub(crate) struct TokenStore {
    pub(crate) inner: Arc<Mutex<HashMap<String, TokenMeta>>>,
    persist: SessionPersister,
}

impl Default for TokenStore {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(dead_code)] // app_name / issued_at persisted for diagnostics, not read at runtime
pub(crate) struct TokenMeta {
    pub(crate) app_name: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) exe_path: PathBuf,
    pub(crate) issued_at: DateTime<Utc>,
    pub(crate) expires_at: DateTime<Utc>,
}

/// On-disk wrapper for the sessions map. Keyed by token so the JSON
/// shape mirrors the in-memory `HashMap` exactly.
#[derive(Serialize, Deserialize)]
pub(crate) struct SessionsFile {
    pub(crate) version: u32,
    pub(crate) sessions: HashMap<String, TokenMeta>,
}

impl TokenStore {
    /// Load any persisted sessions from the keyring, prune anything
    /// already past its `expires_at`, and write the cleaned set back if
    /// pruning removed entries.
    ///
    /// A *missing entry*, parse error, or version mismatch is recoverable
    /// and yields an empty store — those are data conditions, not keyring
    /// faults. An *unavailable keyring* (no secret service, or a locked
    /// keyring whose unlock prompt was dismissed), however, is fatal: the
    /// daemon refuses to start so it never runs unable to persist or
    /// validate sessions. Set [`ALLOW_NO_KEYRING_ENV`] to downgrade that
    /// to an ephemeral in-memory store for headless / CI hosts.
    ///
    /// # Errors
    /// Returns an error when the system keyring is unavailable and
    /// [`ALLOW_NO_KEYRING_ENV`] is not set, so the daemon aborts startup.
    /// An empty store with a live persist task (production) / no-op persister
    /// (test builds).
    fn empty() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            persist: SessionPersister::spawn(),
        }
    }

    pub(crate) fn load_persisted() -> anyhow::Result<Self> {
        let store = Self::empty();

        let blob = match crate::keyring::get_sessions_blob() {
            Ok(Some(b)) => b,
            Ok(None) => {
                info!("No persisted sessions found; starting with empty store");
                return Ok(store);
            }
            Err(e) if allow_no_keyring() => {
                warn!(
                    "System keyring unavailable ({e}); {ALLOW_NO_KEYRING_ENV}=1 set — \
                     starting with an ephemeral in-memory session store (sessions will \
                     not survive a daemon restart)"
                );
                return Ok(store);
            }
            Err(e) => {
                error!(
                    "System keyring is unavailable or missing ({e}). Super STT requires \
                     an accessible secret-service keyring to persist sessions and will \
                     not start without one. Unlock or enable your keyring, or set \
                     {ALLOW_NO_KEYRING_ENV}=1 to start without persistence (headless/CI)."
                );
                return Err(anyhow::anyhow!("keyring unavailable: {e}"));
            }
        };

        let parsed: SessionsFile = match serde_json::from_str(&blob) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to parse persisted sessions ({e}); starting with empty store");
                return Ok(store);
            }
        };

        if parsed.version != SESSIONS_SCHEMA_VERSION {
            warn!(
                "Persisted sessions schema version {} != expected {}; ignoring",
                parsed.version, SESSIONS_SCHEMA_VERSION
            );
            return Ok(store);
        }

        let now = Utc::now();
        let total = parsed.sessions.len();
        let live: HashMap<String, TokenMeta> = parsed
            .sessions
            .into_iter()
            .filter(|(_, meta)| meta.expires_at > now)
            .collect();
        let pruned = total - live.len();

        info!(
            "Loaded {} persisted sessions ({pruned} expired pruned)",
            live.len()
        );

        if pruned > 0 {
            // Write the cleaned map back so disk state matches memory.
            let cleaned = SessionsFile {
                version: SESSIONS_SCHEMA_VERSION,
                sessions: live.clone(),
            };
            if let Ok(json) = serde_json::to_string(&cleaned)
                && let Err(e) = crate::keyring::set_sessions_blob(&json)
            {
                warn!("Failed to persist pruned sessions blob: {e}");
            }
        }

        *store.inner.lock().unwrap() = live;
        Ok(store)
    }

    /// Mint a fresh 30-day session token for `(app_name, scopes, exe_path)`,
    /// insert it into the in-memory map, and submit the updated snapshot to the
    /// persist task. The submit happens under the lock so the persist channel's
    /// order matches lock order (audit 2 Tier 1 #4). Persistence failures are
    /// logged but not propagated — the in-memory map stays authoritative for the
    /// lifetime of the daemon.
    ///
    /// **Failure suppression.** A locked or denied keyring will fail
    /// every write; without a cooldown, a busy session-mint loop would
    /// re-prompt the user every few seconds. After one failure we
    /// suppress further writes for `KEYRING_FAILURE_COOLDOWN`. The
    /// daemon's in-memory map remains correct; we just lose
    /// persistence for that window. A subsequent successful write
    /// (e.g. user unlocks the keyring) clears the suppression flag.
    ///
    /// In `cargo test` builds this is a no-op so unit tests don't
    /// pollute or depend on the developer's real keyring. End-to-end
    /// behavior across daemon restarts is covered by the integration
    /// smoke test in `tests/http_smoke_full.rs` (which exercises this
    /// crate built without the `test` cfg flag).
    ///
    pub(crate) fn mint(
        &self,
        app_name: &str,
        scopes: &[String],
        exe_path: &Path,
    ) -> (String, DateTime<Utc>) {
        let token = generate_token();
        let now = Utc::now();
        let expires_at = now + ChronoDuration::days(30);
        let meta = TokenMeta {
            app_name: app_name.to_string(),
            scopes: scopes.to_vec(),
            exe_path: exe_path.to_path_buf(),
            issued_at: now,
            expires_at,
        };
        {
            let mut tokens = self.inner.lock().unwrap();
            tokens.insert(token.clone(), meta);
            // Submit under the lock so the persist channel's order matches lock
            // order. An off-lock submit can invert (two mutations serialize on
            // the Mutex yet reach the channel out of order) and the coalescer
            // would then write a stale subset snapshot — resurrecting a revoked
            // token across restart. `submit` is a non-blocking channel send (the
            // keyring I/O happens off-thread in the persist task), so it's safe
            // to hold the std Mutex across it (audit 2 Tier 1 #4).
            self.persist.submit(tokens.clone());
        }
        (token, expires_at)
    }

    pub(crate) fn validate(&self, token: &str) -> Result<TokenMeta, &'static str> {
        // Submit the expired-eviction snapshot under the lock so the persist
        // channel's order matches lock order (audit 2 Tier 1 #4). `submit` is a
        // non-blocking channel send — the keyring DBus I/O happens off-thread in
        // the persist task, not here — so it's safe to hold the std Mutex across
        // it. The write is best-effort: the in-memory state is already correct
        // and the cooldown mechanism handles transient keyring failures.
        let mut tokens = self.inner.lock().unwrap();
        let Some(meta) = tokens.get(token).cloned() else {
            return Err("unknown");
        };
        if meta.expires_at < Utc::now() {
            tokens.remove(token);
            self.persist.submit(tokens.clone());
            return Err("expired");
        }
        Ok(meta)
    }

    /// Drop a session token immediately and persist the change. Used by
    /// the `/events` exe-watch path on `exe_changed`: a binary
    /// replacement during a long-lived widget connection invalidates
    /// the session, so the daemon revokes the token, emits a `revoked`
    /// SSE event, and closes the stream. Idempotent.
    pub(crate) fn revoke(&self, token: &str) {
        let mut tokens = self.inner.lock().unwrap();
        if tokens.remove(token).is_some() {
            // Submit under the lock — see `mint` (audit 2 Tier 1 #4).
            self.persist.submit(tokens.clone());
        }
    }
}

pub(crate) fn generate_token() -> String {
    use std::fmt::Write as _;
    let rng = SystemRandom::new();
    let mut bytes = [0u8; 32];
    rng.fill(&mut bytes).expect("SystemRandom::fill");
    let mut s = String::with_capacity(64);
    for b in bytes {
        write!(&mut s, "{b:02x}").expect("string write");
    }
    s
}

#[cfg(test)]
mod tests {
    //! Session-token security semantics: expiry + eviction, the
    //! unknown-token path, mint correctness, and revocation. `validate`
    //! and `revoke` are the gates every authenticated request and the
    //! `/events` exe-watch path rely on, so they're exercised directly
    //! here. Under `cfg!(test)` no persist task is spawned and the
    //! `SessionPersister` receiver is dropped, so mint/revoke submissions
    //! are no-ops — none of these touch the system keyring.
    use super::{TokenMeta, TokenStore};
    use chrono::{Duration as ChronoDuration, Utc};
    use std::path::PathBuf;

    /// Insert a token straight into the in-memory map with an arbitrary
    /// `expires_at`, bypassing `mint`'s fixed 30-day TTL so we can pin
    /// the expiry boundary.
    fn insert_with_expiry(store: &TokenStore, token: &str, expires_at: chrono::DateTime<Utc>) {
        let meta = TokenMeta {
            app_name: "test-app".to_string(),
            scopes: vec!["status".to_string()],
            exe_path: PathBuf::from("/usr/bin/test-client"),
            issued_at: Utc::now() - ChronoDuration::days(1),
            expires_at,
        };
        store.inner.lock().unwrap().insert(token.to_string(), meta);
    }

    /// A freshly minted token validates, carries the scopes + exe it was
    /// minted with, gets a ~30-day expiry, and is a 64-char hex string.
    /// Distinct mints never collide.
    #[test]
    fn mint_then_validate_roundtrips() {
        let store = TokenStore::default();
        let scopes = vec!["transcribe".to_string(), "status".to_string()];
        let exe = PathBuf::from("/usr/bin/super-stt-cli");

        let (token, expires_at) = store.mint("Super STT CLI", &scopes, &exe);
        assert_eq!(token.len(), 64, "token is 32 random bytes hex-encoded");
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "token must be lowercase hex: {token}"
        );

        // ~30 days out (allow a day of slack on each side for clock/runtime).
        let now = Utc::now();
        assert!(
            expires_at > now + ChronoDuration::days(29)
                && expires_at < now + ChronoDuration::days(31),
            "mint should set a ~30-day expiry, got {expires_at}"
        );

        let meta = store.validate(&token).expect("live token must validate");
        assert_eq!(meta.scopes, scopes, "validated scopes match minted scopes");
        assert_eq!(meta.exe_path, exe, "validated exe matches minted exe");

        // A second mint yields a different token.
        let (token2, _) = store.mint("Super STT CLI", &scopes, &exe);
        assert_ne!(token, token2, "each mint produces a unique token");
    }

    /// A token unknown to the store is rejected with the `unknown`
    /// reason (this becomes `401 invalid_session (unknown)` on the wire).
    #[test]
    fn validate_unknown_token_is_unknown() {
        let store = TokenStore::default();
        assert!(
            matches!(store.validate("never-minted"), Err("unknown")),
            "an unknown token must be rejected as `unknown`"
        );
    }

    /// An expired token is rejected with `expired` AND evicted from the
    /// store, so a second presentation reports `unknown` rather than
    /// `expired` — the daemon never keeps a dead token around.
    #[test]
    fn expired_token_is_rejected_and_evicted() {
        let store = TokenStore::default();
        insert_with_expiry(&store, "stale", Utc::now() - ChronoDuration::seconds(1));

        assert!(
            matches!(store.validate("stale"), Err("expired")),
            "a token past its expiry must be rejected as `expired`"
        );
        assert!(
            !store.inner.lock().unwrap().contains_key("stale"),
            "an expired token must be evicted on validation"
        );
        assert!(
            matches!(store.validate("stale"), Err("unknown")),
            "the evicted token is now simply unknown"
        );
    }

    /// A token whose expiry is still in the future validates cleanly —
    /// the expiry check is strictly `expires_at < now`.
    #[test]
    fn not_yet_expired_token_validates() {
        let store = TokenStore::default();
        insert_with_expiry(&store, "fresh", Utc::now() + ChronoDuration::minutes(1));
        assert!(
            store.validate("fresh").is_ok(),
            "a token with a future expiry must validate"
        );
    }

    /// `revoke` immediately invalidates a live token. This is the
    /// primitive the `/events` exe-watch task calls on `exe_changed`
    /// (a binary swap during a long-lived widget connection), so after
    /// revocation the session is gone and any further use is `unknown`.
    #[test]
    fn revoke_invalidates_a_live_token() {
        let store = TokenStore::default();
        let (token, _) = store.mint(
            "widget",
            &["status".to_string()],
            &PathBuf::from("/usr/bin/w"),
        );
        assert!(store.validate(&token).is_ok(), "token valid before revoke");

        store.revoke(&token);
        assert!(
            matches!(store.validate(&token), Err("unknown")),
            "a revoked token must no longer validate"
        );
    }

    /// `revoke` is idempotent: revoking an unknown token (or the same
    /// token twice) is a harmless no-op.
    #[test]
    fn revoke_is_idempotent() {
        let store = TokenStore::default();
        store.revoke("never-existed"); // must not panic
        let (token, _) = store.mint("app", &["status".to_string()], &PathBuf::from("/bin/a"));
        store.revoke(&token);
        store.revoke(&token); // second revoke is still a no-op
        assert!(matches!(store.validate(&token), Err("unknown")));
    }
}
