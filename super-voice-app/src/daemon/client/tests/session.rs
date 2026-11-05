// SPDX-License-Identifier: GPL-3.0-only
//! The session-token machinery every `v1/` wrapper runs inside: the bearer
//! header it attaches, and the one re-auth it is allowed on a rejected token.

use serde_json::json;

use super::fake_daemon::{self, FakeDaemon, Reply};
use crate::daemon::client::v1::{ping, settings};

/// Every call carries the cached token. It is the whole authorization story —
/// the daemon has no other way to tell the settings app from anything else that
/// can reach the socket.
#[tokio::test]
async fn every_call_presents_the_cached_token() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(
            &json!({ "status": "success", "message": "pong" }),
        ))
        .reply(Reply::ok());

    ping::ping_daemon().await.expect("ping");
    settings::volume::set_volume(50).await.expect("volume set");

    for request in daemon.requests() {
        assert_eq!(
            request.authorization.as_deref(),
            Some(format!("Bearer {}", fake_daemon::TOKEN).as_str()),
            "{} {} went out unauthenticated",
            request.method,
            request.target
        );
    }
}

/// A token the daemon rejects is a cache that has gone stale — revoked, or
/// expired while the app sat open. The cache is dropped, consent is asked for
/// again, and the call is retried once. A client that skips the invalidation
/// step instead presents the same dead token on every call for the rest of the
/// process's life, and every settings read reports `invalid_session` until the
/// app is restarted.
#[tokio::test]
async fn a_rejected_token_is_re_minted_and_the_call_retried_once() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::status(
            401,
            &json!({ "status": "error", "data": { "reason": "expired" } }),
        ))
        .reply(Reply::json(&json!({
            "session_token": "a-fresh-token",
            "scopes": ["settings"],
            "expires_at": "2026-09-15T00:00:00Z",
        })))
        .reply(Reply::json(
            &json!({ "status": "success", "message": "pong" }),
        ));

    let greeting = ping::ping_daemon().await.expect("the retry succeeds");

    let requests = daemon.requests();
    assert_eq!(requests.len(), 3, "rejected, re-authed, retried");
    assert_eq!(requests[0].path(), "/ping");
    assert_eq!(
        requests[1].path(),
        "/auth/request",
        "the stale token is not simply reused"
    );
    assert_eq!(
        requests[1].authorization, None,
        "minting a token cannot require one"
    );
    assert_eq!(requests[2].path(), "/ping");
    assert_eq!(
        requests[2].authorization.as_deref(),
        Some("Bearer a-fresh-token"),
        "the retry presents the token that was just minted"
    );
    assert_eq!(greeting, "pong");
}

/// The retry is once, not a loop: a daemon that rejects the freshly minted
/// token too has a real problem, and hammering it with consent popups is not
/// how the user finds out.
#[tokio::test]
async fn a_second_rejection_is_reported_rather_than_retried_again() {
    let daemon = FakeDaemon::start().await;
    let rejection = json!({ "status": "error", "data": { "reason": "expired" } });
    daemon
        .reply(Reply::status(401, &rejection))
        .reply(Reply::json(&json!({
            "session_token": "a-fresh-token",
            "scopes": ["settings"],
            "expires_at": "2026-09-15T00:00:00Z",
        })))
        .reply(Reply::status(401, &rejection));

    let error = ping::ping_daemon().await.expect_err("still rejected");

    assert_eq!(daemon.requests().len(), 3, "no third attempt");
    assert!(error.to_string().contains("expired"), "got {error}");
}

/// A user who closes the consent popup gets the refusal, not a retry loop and
/// not a daemon error — the distinction matters because only one of them is
/// something the app should offer to try again.
#[tokio::test]
async fn a_declined_consent_comes_back_as_a_denial() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::status(
            401,
            &json!({ "status": "error", "data": { "reason": "revoked" } }),
        ))
        .reply(Reply::status(
            403,
            &json!({ "status": "error", "data": { "reason": "user_denied" } }),
        ));

    let error = ping::ping_daemon().await.expect_err("consent declined");

    assert!(error.to_string().contains("user_denied"), "got {error}");
}

/// Only `401` re-auths. Any other failure is about the request, not the token,
/// and asking the user for consent again would be theatre.
#[tokio::test]
async fn a_failure_that_is_not_about_the_token_does_not_re_auth() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::status(
        503,
        &json!({ "status": "error", "message": "still starting up" }),
    ));

    let error = ping::ping_daemon().await.expect_err("daemon unavailable");

    assert_eq!(daemon.request().path(), "/ping");
    assert_eq!(error.to_string(), "still starting up (HTTP 503)");
}
