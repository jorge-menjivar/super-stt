// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::auth::consent::{
    ConsentDecision, ConsentKey, ask_user_for_consent, normalize_scopes, resolve_peer_exe,
};
use crate::daemon::http::internal::helpers::responses::{auth_err, is_known_scope, reason};
use crate::daemon::http::state::{AppState, PeerInfo};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Deserialize, Debug)]
pub(crate) struct AuthRequestBody {
    pub(crate) app_name: String,
    pub(crate) scopes: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)] // accepted for forwards compat; not yet used
    pub(crate) version: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct AuthOk {
    pub(crate) status: &'static str,
    pub(crate) session_token: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) expires_at: String,
}

pub(crate) async fn auth_request(
    State(state): State<AppState>,
    peer: Option<axum::Extension<PeerInfo>>,
    body: Option<axum::Json<AuthRequestBody>>,
) -> Response {
    let Some(axum::Json(body)) = body else {
        return auth_err(StatusCode::BAD_REQUEST, "auth_denied", reason::INVALID_BODY);
    };

    // Validate the requested scope set: non-empty and every entry a known
    // scope. Normalize (sort + dedup) so the consent key and granted set
    // don't depend on the order the client listed them.
    if body.scopes.is_empty() || !body.scopes.iter().all(|s| is_known_scope(s)) {
        return auth_err(
            StatusCode::BAD_REQUEST,
            "auth_denied",
            reason::INVALID_SCOPE,
        );
    }
    let scopes = normalize_scopes(&body.scopes);

    // Reject same-host, different-user peers BEFORE spawning a popup
    // or touching consent state. Socket perms 0o660 + group `stt`
    // mean a second user in that group can otherwise pop dialogs on
    // the daemon owner's desktop in another app's name. peer_cred is
    // None only on platforms without SO_PEERCRED — treat that as
    // safe-fail-closed since we expect Linux.
    let daemon_uid = unsafe { libc::geteuid() };
    let peer_uid = peer.as_ref().and_then(|p| p.0.uid);
    match peer_uid {
        Some(uid) if uid == daemon_uid => {}
        Some(uid) => {
            log::warn!(
                "auth_request rejected: peer uid {uid} differs from daemon uid {daemon_uid}"
            );
            return auth_err(StatusCode::FORBIDDEN, "auth_denied", reason::UID_MISMATCH);
        }
        None => {
            log::warn!("auth_request rejected: peer uid unavailable");
            return auth_err(StatusCode::FORBIDDEN, "auth_denied", reason::UID_MISMATCH);
        }
    }

    // Resolve the calling binary via /proc/<pid>/exe. Log each
    // failure mode separately so we can tell whether the issue is
    // missing peer credentials, a missing pid, or a kernel/proc
    // permission denial. Falls back to "<unknown>" so the popup still
    // has something to display.
    let exe_path = resolve_peer_exe(peer.as_ref());

    // Helper to build the success response for a (token, expires_at)
    // pair. Used by both reuse-scan paths (fast and slow) and the
    // post-mint path so they all serialize the same JSON shape.
    let ok_response = |token: String, expires_at: DateTime<Utc>| -> Response {
        let payload = AuthOk {
            status: "success",
            session_token: token,
            scopes: scopes.clone(),
            expires_at: expires_at.to_rfc3339(),
        };
        (
            StatusCode::OK,
            [("content-type", "application/json")],
            serde_json::to_string(&payload).unwrap_or_default(),
        )
            .into_response()
    };

    // `auth_request` always runs the consent flow — no token
    // validation, no identity-only reuse. Clients that have a valid
    // cached token never reach this endpoint; they go straight to
    // `/ping`/`/events`/etc., where the bearer header is validated
    // by `TokenStore::validate`. Clients that hit `auth_request`
    // do so precisely because they need a fresh token, and the
    // consistent semantic is "request fresh consent".

    let consent_key: ConsentKey = (exe_path.clone(), scopes.clone());

    // Sticky-deny short-circuit. If the user previously clicked Deny
    // for this exact (exe_path, scope) pair in this daemon's
    // lifetime, reject immediately without spawning another popup.
    // The cache is cleared by daemon restart; there's no other reset
    // path on purpose. `app_name` is not part of the key because a
    // misbehaving client could otherwise rotate it to bypass deny.
    if state.deny_cache.contains(&consent_key) {
        log::info!(
            "auth_request denied from cache (user previously denied): exe={} scopes={}",
            exe_path.display(),
            scopes.join(" ")
        );
        return auth_err(
            StatusCode::FORBIDDEN,
            "auth_denied",
            reason::USER_DENIED_CACHED,
        );
    }

    // Serialize concurrent first-time requests for the same identity
    // so we don't spawn N consent popups when N clients race. Each
    // racer still gets its own popup, but they happen one at a time
    // rather than stacking on screen. The lock entry is released by
    // `state.consent_locks.release` after the flow completes so the
    // registry doesn't grow unboundedly.
    let lock = state.consent_locks.lock_for(consent_key.clone());
    let response = {
        let _guard = lock.lock().await;

        // Re-check the deny cache: a concurrent caller for the same
        // pair may have been denied while we were queued behind
        // their popup.
        if state.deny_cache.contains(&consent_key) {
            log::info!(
                "auth_request denied from cache (concurrent caller denied): exe={} scopes={}",
                exe_path.display(),
                scopes.join(" ")
            );
            auth_err(
                StatusCode::FORBIDDEN,
                "auth_denied",
                reason::USER_DENIED_CACHED,
            )
        } else {
            // Auto-approve if the daemon is in test/CI mode.
            let auto_approve = std::env::var(crate::daemon::http::server::AUTO_APPROVE_ENV)
                .is_ok_and(|v| v == "1");

            let decision = if auto_approve {
                let auto_approve_env = crate::daemon::http::server::AUTO_APPROVE_ENV;
                log::info!(
                    "{auto_approve_env}=1 set; auto-approving auth_request for {} ({})",
                    body.app_name,
                    exe_path.display()
                );
                ConsentDecision::Allow
            } else {
                ask_user_for_consent(&body.app_name, &scopes, &exe_path).await
            };

            finalize_consent_decision(
                decision,
                &state,
                &body,
                &scopes,
                &exe_path,
                consent_key.clone(),
                &ok_response,
            )
        }
    };

    // Prune the consent-locks registry entry. If another in-flight
    // auth_request is still waiting on the same key, `release` keeps
    // the entry; once everyone is done it gets removed.
    state.consent_locks.release(&consent_key, &lock);

    response
}

/// Resolve a [`ConsentDecision`] into the matching HTTP response,
/// folding in the side effects each branch needs (token mint on
/// Allow, deny-cache insert on Deny). Extracted out of `auth_request`
/// to keep that handler under the workspace's clippy line cap.
pub(crate) fn finalize_consent_decision(
    decision: ConsentDecision,
    state: &AppState,
    body: &AuthRequestBody,
    scopes: &[String],
    exe_path: &Path,
    consent_key: ConsentKey,
    ok_response: &dyn Fn(String, DateTime<Utc>) -> Response,
) -> Response {
    match decision {
        ConsentDecision::Allow => {
            let (token, expires_at) = state.tokens.mint(&body.app_name, scopes, exe_path);
            log::info!(
                "auth_request approved: app={} scopes={}",
                body.app_name,
                scopes.join(" ")
            );
            ok_response(token, expires_at)
        }
        ConsentDecision::Deny => {
            // Sticky: remember this (exe, scopes) pair so the next
            // request from the same binary is auto-denied without
            // re-prompting. In-memory only — daemon restart resets.
            log::info!(
                "auth_request denied by user; caching deny for app={} scopes={}",
                body.app_name,
                scopes.join(" ")
            );
            state.deny_cache.insert(consent_key);
            auth_err(StatusCode::FORBIDDEN, "auth_denied", reason::USER_DENIED)
        }
        ConsentDecision::Dismissed => {
            // *Don't* cache Dismissed (e.g. user closed the popup
            // without making a choice, the helper crashed, etc.).
            // Treat as transient — the next request gets a fresh
            // popup.
            auth_err(StatusCode::FORBIDDEN, "auth_denied", reason::USER_DISMISSED)
        }
        ConsentDecision::PopupFailed => {
            auth_err(StatusCode::FORBIDDEN, "auth_denied", reason::POPUP_FAILED)
        }
    }
}
