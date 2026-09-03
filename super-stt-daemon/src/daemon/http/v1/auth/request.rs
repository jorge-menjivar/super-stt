// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::auth::consent::{
    ConsentDecision, ConsentKey, ask_user_for_consent, is_official_client, normalize_scopes,
    resolve_peer_exe,
};
use crate::daemon::http::internal::helpers::responses::{auth_err, is_known_scope, reason};
use crate::daemon::http::state::{AppState, PeerInfo};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Deserialize, Debug, utoipa::ToSchema)]
pub(crate) struct AuthRequestBody {
    /// The name shown to the user in the consent popup. Self-reported and
    /// therefore untrusted: the daemon identifies you by your binary, and a
    /// previous denial sticks to that binary whatever name you send next.
    #[schema(example = "My App")]
    pub(crate) app_name: String,
    /// The scopes to request, at least one. Every entry must be known or the
    /// whole request is refused; ask only for what you need, since the user
    /// sees the list.
    #[schema(example = json!(["transcribe", "status"]))]
    pub(crate) scopes: Vec<String>,
    /// Your app's version. Accepted for forwards compatibility; unused today.
    #[serde(default)]
    #[allow(dead_code)] // accepted for forwards compat; not yet used
    #[schema(example = "0.1")]
    pub(crate) version: Option<String>,
}

/// A freshly minted session token.
#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct AuthOk {
    /// Always `success`.
    #[schema(example = "success")]
    pub(crate) status: &'static str,
    /// Send this as `Authorization: Bearer <token>` on every other endpoint.
    /// Bound to the approved binary — it stops working if that binary changes
    /// on disk.
    pub(crate) session_token: String,
    /// The scopes actually granted, sorted and deduplicated.
    pub(crate) scopes: Vec<String>,
    /// RFC 3339 expiry, 30 days out.
    pub(crate) expires_at: String,
}

/// `403 user_denied_cached` if the user previously clicked Deny for this exact
/// `(exe_path, scopes)` pair in this daemon's lifetime, else `None`. Checked both
/// up front and again under the consent lock (a concurrent caller may have been
/// denied while we queued behind their popup); `context` labels which.
fn cached_deny_response(
    state: &AppState,
    consent_key: &ConsentKey,
    context: &str,
) -> Option<Response> {
    if !state.deny_cache.contains(consent_key) {
        return None;
    }
    let (exe_path, scopes) = consent_key;
    log::info!(
        "auth_request denied from cache ({context}): exe={} scopes={}",
        exe_path.display(),
        scopes.join(" ")
    );
    Some(auth_err(
        StatusCode::FORBIDDEN,
        "auth_denied",
        reason::USER_DENIED_CACHED,
    ))
}

/// First-party short-circuit for `auth_request`: a trusted co-located
/// client binary skips the popup and mints immediately. `None` means
/// the peer is not first-party and the normal consent flow proceeds.
fn official_client_response(
    state: &AppState,
    body: &AuthRequestBody,
    scopes: &[String],
    exe_path: &Path,
    consent_key: ConsentKey,
    ok_response: &dyn Fn(String, DateTime<Utc>) -> Response,
) -> Option<Response> {
    if !is_official_client(exe_path) {
        return None;
    }
    log::info!(
        "auth_request auto-approved for first-party client: app={} exe={} scopes={}",
        body.app_name,
        exe_path.display(),
        scopes.join(" ")
    );
    Some(finalize_consent_decision(
        ConsentDecision::Allow,
        state,
        body,
        scopes,
        exe_path,
        consent_key,
        ok_response,
    ))
}

#[utoipa::path(
    post,
    path = "/auth/request",
    tag = "auth",
    summary = "Ask the user for a session token",
    description = "\
The consent handshake, and the only endpoint reachable without a token.

The daemon reads `SO_PEERCRED` on your connection, resolves `/proc/<pid>/exe`, and \
shows the user a popup naming that binary and the scopes you asked for. On Allow it \
mints a 32-byte token bound to that binary and valid for 30 days.

A denial is remembered for the `(binary, scopes)` pair for the rest of the daemon's \
lifetime and answers `403` immediately without re-prompting — renaming your app does \
not clear it, since the key is the binary. Restarting the daemon does.

Setting `SUPER_STT_AUTO_APPROVE=1` in the daemon's environment skips the popup \
entirely; it is for tests and CI, not for production.",
    request_body = AuthRequestBody,
    responses(
        (status = 200, description = "The user approved. Store the token.", body = AuthOk),
        (status = 400, description = "Body was missing or malformed (`invalid_body`), or `scopes` was empty or named an unknown scope (`invalid_scope`).", body = ReasonEnvelope),
        (status = 403, description = "\
The user denied or dismissed the popup (`user_denied`, `user_dismissed`), a previous \
denial for this binary and scope set still stands (`user_denied_cached`), the \
connecting user is not the daemon's own (`uid_mismatch`), the daemon could not \
resolve your binary and so refused to identify you (`peer_unverifiable`), or the \
popup could not be shown (`popup_failed`).", body = ReasonEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
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

    // Resolve the calling binary via /proc/<pid>/exe. If it can't be resolved
    // (missing peer credentials/pid, or a kernel/proc permission denial), fail
    // closed: consent verifies a *binary*, so we refuse rather than prompt with
    // an unverifiable `<unknown>` identity or mint a token bound to it (audit 2
    // Tier 3 #9). Each failure mode is logged inside `resolve_peer_exe`.
    let Some(exe_path) = resolve_peer_exe(peer.as_ref()) else {
        log::warn!("auth_request rejected: could not verify the requesting binary (fail-closed)");
        return auth_err(
            StatusCode::FORBIDDEN,
            "auth_denied",
            reason::PEER_UNVERIFIABLE,
        );
    };

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

    // First-party binaries co-located with the daemon skip the popup
    // (docs/protocol/auth.md § First-party clients); downstream is
    // identical to a popup-approved grant. See `is_official_client`
    // for the trust rationale.
    if let Some(resp) = official_client_response(
        &state,
        &body,
        &scopes,
        &exe_path,
        consent_key.clone(),
        &ok_response,
    ) {
        return resp;
    }

    // Sticky-deny short-circuit. If the user previously clicked Deny for this
    // exact (exe_path, scope) pair in this daemon's lifetime, reject immediately
    // without spawning another popup. The cache is cleared by daemon restart;
    // there's no other reset path on purpose. `app_name` is not part of the key
    // because a misbehaving client could otherwise rotate it to bypass deny.
    if let Some(resp) = cached_deny_response(&state, &consent_key, "user previously denied") {
        return resp;
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

        // Re-check the deny cache: a concurrent caller for the same pair may
        // have been denied while we were queued behind their popup.
        if let Some(resp) = cached_deny_response(&state, &consent_key, "concurrent caller denied") {
            resp
        } else {
            // Auto-approve if the daemon is in test/CI mode. Honored only in
            // debug builds: compiled out of release so a stray/injected env var
            // can't silently defeat the human consent gate in a shipped binary
            // (audit 2 Tier 1 #6; mirrors the #30 consent-timer release gating).
            #[cfg(debug_assertions)]
            let auto_approve = std::env::var(crate::daemon::http::server::AUTO_APPROVE_ENV)
                .is_ok_and(|v| v == "1");
            #[cfg(not(debug_assertions))]
            let auto_approve = false;

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
