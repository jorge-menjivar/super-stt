// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::PeerInfo;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Global cap of one on-screen consent popup at a time. See
/// [`ask_user_for_consent`] — without it a same-uid client could drive hundreds
/// of concurrent exclusive-keyboard dialogs (255 distinct consent keys) and lock
/// the desktop (audit 2 Tier 3 #10).
static CONSENT_POPUP: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Identifies the consent flow uniquely: (`exe_path`, normalized
/// `scopes`). The user verifies a *binary*, not a self-reported display
/// name, so the deny / dedup key is keyed on the kernel-resolved
/// `exe_path` plus the requested scope set (sorted + deduped via
/// [`normalize_scopes`] so request order doesn't matter). `app_name` is
/// shown in the popup but isn't part of the identity.
pub(crate) type ConsentKey = (PathBuf, Vec<String>);
pub(crate) type ConsentLock = Arc<tokio::sync::Mutex<()>>;

/// Sort + dedup a requested scope list so the consent key and the
/// granted set are independent of the order the client listed them.
pub(crate) fn normalize_scopes(scopes: &[String]) -> Vec<String> {
    let mut v = scopes.to_vec();
    v.sort();
    v.dedup();
    v
}

/// Per-`(exe_path, scope)` async mutex registry used by the
/// `/auth/request` handler to dedup concurrent first-time consent
/// requests. Without this, two clients that ping the daemon at the same
/// time on a fresh install would each spawn their own consent popup;
/// with it, the second blocks until the first finishes and then
/// short-circuits via the reuse-scan against the now-minted token.
///
/// The map is pruned via [`Self::release`] after the auth flow
/// completes so a malicious client can't drive unbounded memory
/// growth by spamming /auth/request with rotating keys.
#[derive(Clone, Default)]
pub(crate) struct ConsentLocks {
    inner: Arc<Mutex<HashMap<ConsentKey, ConsentLock>>>,
}

impl ConsentLocks {
    pub(crate) fn lock_for(&self, key: ConsentKey) -> ConsentLock {
        let mut map = self.inner.lock().unwrap();
        map.entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Drop the registry entry for `key` if no other task is still
    /// holding the `ConsentLock`. Called from the `auth_request`
    /// handler after the consent flow finishes — success or denial.
    /// `strong_count == 2` means exactly the map and our local clone
    /// hold references; anything higher means another in-flight
    /// `auth_request` for the same key is still waiting on the same
    /// mutex and we leave the entry in place for it.
    pub(crate) fn release(&self, key: &ConsentKey, lock: &ConsentLock) {
        let mut map = self.inner.lock().unwrap();
        // Our `lock` reference plus the one inside the map. If
        // anything else is still holding, leave it.
        if Arc::strong_count(lock) <= 2 {
            map.remove(key);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ConsentDecision {
    Allow,
    Deny,
    Dismissed,
    PopupFailed,
}

/// Spawn the `super-stt-consent` helper binary, wait up to 60s for the
/// user's decision. The helper writes one of `allow` / `deny` / `dismissed`
/// to stdout and exits.
/// Read the consent helper's single-line verdict from its stdout.
async fn read_consent_decision(stdout: tokio::process::ChildStdout) -> ConsentDecision {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut reader = BufReader::new(stdout).lines();
    match reader.next_line().await {
        Ok(Some(line)) => match line.trim() {
            "allow" => ConsentDecision::Allow,
            "deny" => ConsentDecision::Deny,
            _ => ConsentDecision::Dismissed,
        },
        _ => ConsentDecision::Dismissed,
    }
}

pub(crate) async fn ask_user_for_consent(
    app_name: &str,
    scopes: &[String],
    exe_path: &Path,
) -> ConsentDecision {
    // `locate_consent_helper` already logs a specific reason on every
    // failure path (missing / un-canonicalizable / failed metadata check),
    // so we don't emit a second, redundant warning here.
    let Some(helper) = locate_consent_helper() else {
        return ConsentDecision::PopupFailed;
    };

    // Serialize popups globally: at most one consent dialog on screen at a time
    // (audit 2 Tier 3 #10). `/auth/request` is unauthenticated and outside the
    // rate limiter, and the 8 scopes yield 255 distinct `(exe, scopes)` consent
    // keys — each bypassing the per-key dedup — so without this cap a same-uid
    // process could stack hundreds of concurrent exclusive-keyboard overlays and
    // lock the desktop. Excess requests wait for the permit rather than opening
    // in parallel. Acquired before the spawn and held while the dialog is on
    // screen; released before the untimed reap below so a wedged helper can't
    // wedge all consent.
    let Ok(popup_permit) = CONSENT_POPUP.acquire().await else {
        return ConsentDecision::PopupFailed; // semaphore closed (never in practice)
    };

    let mut cmd = tokio::process::Command::new(&helper);
    cmd.env("STT_AUTH_APP_NAME", app_name)
        .env("STT_AUTH_SCOPES", scopes.join(" "))
        .env("STT_AUTH_EXE_PATH", exe_path.to_string_lossy().as_ref())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("failed to spawn super-stt-consent: {e}");
            return ConsentDecision::PopupFailed;
        }
    };

    let Some(stdout) = child.stdout.take() else {
        return ConsentDecision::PopupFailed;
    };

    let result = tokio::time::timeout(Duration::from_mins(1), read_consent_decision(stdout)).await;
    let _ = child.start_kill();
    // The dialog is being torn down, so release the global one-popup permit
    // *before* the reap. `child.wait()` is untimed; holding the sole global
    // permit across it would let a helper that somehow doesn't reap promptly
    // (a pathological uninterruptible-sleep) wedge all consent daemon-wide.
    // Releasing first keeps the popup cap intact while the reap still completes.
    drop(popup_permit);
    let _ = child.wait().await;

    result.unwrap_or(ConsentDecision::Dismissed)
}

/// Find the consent helper.
///
/// **Security model.** The helper is only ever looked for **alongside the
/// daemon binary itself**. We deliberately do NOT fall back to `PATH`
/// because doing so would let any attacker who can prepend a writable
/// directory to the daemon's `PATH` (a classic privilege-escalation
/// vector) substitute their own helper. Forcing co-location bounds the
/// attack surface to "whoever can write to the directory holding the
/// daemon binary" — which is the same threshold required to replace the
/// daemon itself, so we don't make consent any easier to subvert than
/// the daemon's own integrity.
///
/// On top of that, before returning the path:
/// - We `canonicalize()` it, so symlink-swap shenanigans don't help.
/// - We verify the resolved file is owned by the daemon's effective uid
///   (catches "someone dropped a helper they own into the install dir").
/// - We verify it isn't world-writable.
pub(crate) fn locate_consent_helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join("super-stt-consent");
    if !candidate.exists() {
        log::warn!(
            "super-stt-consent not found alongside daemon binary at {}; \
             auth_request will be denied with popup_failed",
            candidate.display()
        );
        return None;
    }

    let resolved = match candidate.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            log::warn!(
                "failed to canonicalize consent helper path {}: {e}",
                candidate.display()
            );
            return None;
        }
    };

    if let Err(reason) = verify_helper_metadata(&resolved) {
        log::warn!(
            "consent helper at {} rejected: {reason}",
            resolved.display()
        );
        return None;
    }

    Some(resolved)
}

/// Verify the helper's file metadata is consistent with "trusted binary
/// installed by the user". Returns Err with a static reason on
/// rejection.
#[cfg(unix)]
fn verify_helper_metadata(path: &Path) -> Result<(), &'static str> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).map_err(|_| "cannot stat helper")?;
    // Require the helper to be owned by the same uid the daemon runs
    // as. This catches the case of another local user (or root)
    // dropping a binary into the install dir.
    let our_uid = unsafe { libc::geteuid() };
    if metadata.uid() != our_uid {
        return Err("helper not owned by daemon's effective uid");
    }
    // Reject world-writable helpers — anyone could swap them out.
    if metadata.mode() & 0o002 != 0 {
        return Err("helper is world-writable");
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_helper_metadata(_: &Path) -> Result<(), &'static str> {
    Ok(())
}

/// Resolve the calling process's executable path from the
/// `axum::Extension<PeerInfo>` attached by the accept loop. Returns `Some(path)`
/// on success and `None` when the peer can't be identified — a missing
/// `PeerInfo`/pid (`SO_PEERCRED` unsupported, peer process gone) or a
/// kernel-denied `/proc/<pid>/exe` readlink (Yama `ptrace_scope`, systemd
/// `ProtectProc=`, a sandboxed daemon, pid recycling).
///
/// The caller **must fail closed** on `None`: the consent model verifies a
/// *binary*, so an unidentifiable peer must not be prompted for (a
/// `<unknown>`-labelled dialog is meaningless to approve) nor minted a token
/// bound to a bogus identity that the `/events` exe-watch would then spuriously
/// revoke (audit 2 Tier 3 #9). Each failure is logged with its specific reason.
pub(crate) fn resolve_peer_exe(peer: Option<&axum::Extension<PeerInfo>>) -> Option<PathBuf> {
    let Some(peer) = peer else {
        log::warn!(
            "auth_request: no PeerInfo extension attached — cannot identify the requesting binary"
        );
        return None;
    };
    let Some(pid) = peer.0.pid else {
        log::warn!(
            "auth_request: PeerInfo had no pid (SO_PEERCRED returned no credentials); cannot resolve exe"
        );
        return None;
    };
    let path = format!("/proc/{pid}/exe");
    match std::fs::read_link(&path) {
        Ok(p) => Some(p),
        Err(e) => {
            log::warn!(
                "auth_request: read_link({path}) failed: {e}; cannot identify peer pid {pid}"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_sorts_and_dedups() {
        let got = normalize_scopes(&[
            "transcribe".to_string(),
            "status".to_string(),
            "transcribe".to_string(),
        ]);
        assert_eq!(got, vec!["status".to_string(), "transcribe".to_string()]);
    }

    #[test]
    fn normalize_is_order_independent() {
        let a = normalize_scopes(&["settings".to_string(), "status".to_string()]);
        let b = normalize_scopes(&["status".to_string(), "settings".to_string()]);
        assert_eq!(a, b, "request order must not change the consent key");
    }
}
