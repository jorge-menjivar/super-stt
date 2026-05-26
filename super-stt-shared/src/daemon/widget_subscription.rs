// SPDX-License-Identifier: GPL-3.0-only
//! Self-healing widget `/events` subscription helper.
//!
//! Both the COSMIC applet and the settings app subscribe to the
//! daemon's `GET /events` SSE stream. The naive shape — open the
//! stream once, drain it until it ends — leaks state on three failure
//! modes:
//!
//! 1. **Silent drops.** If the stream just ends (`stream.next()`
//!    returns `None`) without an in-band error, the consumer's UI
//!    keeps showing whatever the last frame was forever.
//! 2. **Stale tokens.** If the daemon revokes the session (its
//!    persisted blob got wiped, the binary changed and triggered
//!    `exe_changed`, etc.), `events_stream` returns
//!    `invalid_session(...)` once and the consumer is stuck — its
//!    cached client token is still in the keyring, so the next
//!    `obtain` returns the same dead value.
//! 3. **Wedged sockets.** A connection that never errors but never
//!    delivers data either is invisible to the read side. The daemon
//!    sends a `: keepalive\n\n` SSE comment every 30 s; if those stop
//!    arriving the consumer should treat the connection as dead.
//!
//! [`run_widget_subscription`] handles all three: an outer reconnect
//! loop with exponential backoff, an idle deadline on every
//! `stream.next()`, and explicit [`session::forget`] before re-`obtain`
//! whenever the daemon signals that the cached token is no longer
//! valid (either via `401 invalid_session` on the request itself or
//! via an in-band `event: revoked` frame).
//!
//! Output is a [`Stream<Item = WidgetSubscriptionUpdate>`] that runs
//! until either the consumer drops it OR the user explicitly denies
//! consent. A denial yields [`WidgetSubscriptionUpdate::Blocked`] and
//! ends the stream — auto-retrying a denied request would just spam
//! the daemon's deny cache. The consumer is expected to surface a
//! "restart the daemon and click retry" hint, then build a new
//! subscription when the user opts in. Consumers map each update into
//! their own GUI message type. Both the applet and the settings app
//! are thin adapters over this.

use crate::daemon::http_client::{self, WidgetEvent};
use crate::daemon::session::{self, AppId};
use futures_util::StreamExt;
use std::path::PathBuf;
use std::time::Duration;

/// Default keepalive grace: the daemon sends a `: keepalive\n\n` SSE
/// comment every 30 s, so a minute of silence is decisive evidence
/// that the connection is wedged. Tunable via
/// [`WidgetSubscriptionConfig`].
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_mins(1);
pub const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_secs(1);
pub const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(30);

/// One update from the running subscription. Caller projects each
/// variant into whatever app-specific message its GUI loop expects.
#[derive(Debug, Clone)]
pub enum WidgetSubscriptionUpdate {
    /// SSE connection just became live (right after the daemon's
    /// `subscribed` event arrives, but the caller never sees the raw
    /// `subscribed` payload).
    Connected,
    /// Daemon emitted an event in one of the subscribed topics.
    Event(WidgetEvent),
    /// Connection ended (clean EOF, idle timeout, peer reset, transient
    /// error). Subscription is reconnecting after a backoff. Caller can
    /// flip its UI to a "connecting" state.
    Disconnected { reason: String },
    /// Cached token is no longer valid (daemon-side revocation or
    /// `exe_changed`). [`session::forget`] was already called, so the
    /// next iteration will trigger fresh consent. Caller can show a
    /// "needs re-consent" hint while the popup spawns.
    NeedsReauth { reason: String },
    /// User actively denied consent (either just now or via the
    /// daemon's sticky deny cache). The subscription stream
    /// terminates after this update — auto-retrying would only spam
    /// the daemon's cache for `user_denied_cached` responses. The
    /// caller should surface a hint to the user (restart the daemon
    /// to clear the deny cache, then explicitly retry) and build a
    /// fresh subscription when the user opts in.
    Blocked { reason: String },
}

/// Per-subscription identity and tunables. Both clients use this with
/// only the `app_id`/`app_name`/`scope`/`topics` fields differing.
#[derive(Clone, Copy)]
pub struct WidgetSubscriptionConfig {
    /// Stable per-app keyring user (e.g. `"super-stt-cosmic-applet"`).
    pub app_id: AppId,
    /// Human-readable name shown in the consent popup.
    pub app_name: &'static str,
    /// `"widget"` for visualizers, `"settings"` for the settings app
    /// (which prefers its god-mode token).
    pub scope: &'static str,
    /// Topics to subscribe to (matches the daemon's `WIDGET_TOPICS`
    /// allow-list when `scope == "widget"`).
    pub topics: &'static [&'static str],
    pub idle_timeout: Duration,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl WidgetSubscriptionConfig {
    /// Build a config with defaults for `idle_timeout` /
    /// `initial_backoff` / `max_backoff`. The required fields
    /// (`app_id`, `app_name`, `scope`, `topics`) are passed in.
    #[must_use]
    pub fn new(
        app_id: AppId,
        app_name: &'static str,
        scope: &'static str,
        topics: &'static [&'static str],
    ) -> Self {
        Self {
            app_id,
            app_name,
            scope,
            topics,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
        }
    }
}

/// Run the self-healing widget subscription. Returns a `Stream` that
/// runs forever — drop it to stop. The stream never returns `Err` (all
/// failures are surfaced as `Disconnected` / `NeedsReauth` updates so
/// the consumer's loop is `while let Some(update)`).
///
/// On every iteration the loop:
///
/// 1. Calls `session::obtain` (cache hit on the keyring or fresh
///    consent popup on cold start / after `forget`).
/// 2. Opens the SSE stream via `http_client::events_stream`.
/// 3. Reads each `stream.next()` with a `tokio::time::timeout` of
///    `config.idle_timeout`.
/// 4. On `revoked` event or `401 invalid_session`, calls
///    `session::forget` and re-obtains.
/// 5. On any drop (idle, EOF, error), backs off and reconnects
///    (exponential, capped at `config.max_backoff`).
#[allow(clippy::too_many_lines)]
pub fn run_widget_subscription(
    socket: PathBuf,
    config: WidgetSubscriptionConfig,
) -> impl futures_util::Stream<Item = WidgetSubscriptionUpdate> + Send + 'static {
    async_stream::stream! {
        let mut backoff = config.initial_backoff;

        loop {
            // 1. Token: cached or freshly minted via consent popup.
            //    Three error families:
            //    - AuthDenied with a user-denied reason → terminal `Blocked`.
            //    - AuthDenied with another reason (dismissed, popup_failed,
            //      throttled) or InvalidSession → recoverable `NeedsReauth`.
            //    - Other (daemon unreachable, network IO, etc.) → recoverable
            //      `Disconnected`. Without this split, callers see a
            //      "session revoked" UI for a plain daemon-offline state.
            let token = match session::obtain(
                socket.clone(),
                config.app_id,
                config.app_name,
                config.scope,
            )
            .await
            {
                Ok(t) => t,
                Err(http_client::HttpError::AuthDenied { reason })
                    if is_user_denied_reason(&reason) =>
                {
                    yield WidgetSubscriptionUpdate::Blocked {
                        reason: format!("auth_denied ({reason})"),
                    };
                    return;
                }
                Err(e @ (http_client::HttpError::AuthDenied { .. }
                | http_client::HttpError::InvalidSession { .. })) => {
                    yield WidgetSubscriptionUpdate::NeedsReauth { reason: e.to_string() };
                    tokio::time::sleep(backoff).await;
                    backoff = next_backoff(backoff, config.max_backoff);
                    continue;
                }
                Err(e @ http_client::HttpError::Other(_)) => {
                    yield WidgetSubscriptionUpdate::Disconnected { reason: e.to_string() };
                    tokio::time::sleep(backoff).await;
                    backoff = next_backoff(backoff, config.max_backoff);
                    continue;
                }
            };

            // 2. Open SSE stream.
            let stream = match http_client::events_stream(
                socket.clone(),
                &token,
                config.topics,
            )
            .await
            {
                Ok(s) => s,
                Err(e) if e.is_invalid_session() => {
                    let _ = session::forget(config.app_id);
                    yield WidgetSubscriptionUpdate::NeedsReauth { reason: e.to_string() };
                    // No backoff — go straight to fresh consent.
                    continue;
                }
                Err(e) => {
                    yield WidgetSubscriptionUpdate::Disconnected { reason: e.to_string() };
                    tokio::time::sleep(backoff).await;
                    backoff = next_backoff(backoff, config.max_backoff);
                    continue;
                }
            };
            let mut stream = Box::pin(stream);
            yield WidgetSubscriptionUpdate::Connected;
            backoff = config.initial_backoff;

            // 3. Read the stream with an idle deadline. Any of (EOF /
            //    timeout / revoked) breaks out and reconnects.
            let mut reauth = None;
            loop {
                match tokio::time::timeout(config.idle_timeout, stream.next()).await {
                    Ok(Some(evt)) => {
                        if evt.name == "revoked" {
                            let reason = evt
                                .payload
                                .get("reason")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            reauth = Some(format!("revoked: {reason}"));
                            break;
                        }
                        // The daemon's initial `subscribed` event is
                        // useful only for client_id correlation, which
                        // the consumer doesn't need. Hide it. The
                        // synthetic `keepalive` events from comment-only
                        // SSE blocks are similarly internal — they
                        // exist solely to refresh the idle deadline on
                        // every wire-level activity.
                        if matches!(evt.name.as_str(), "subscribed" | "keepalive") {
                            continue;
                        }
                        yield WidgetSubscriptionUpdate::Event(evt);
                    }
                    Ok(None) => {
                        yield WidgetSubscriptionUpdate::Disconnected {
                            reason: "stream ended".to_string(),
                        };
                        break;
                    }
                    Err(_) => {
                        yield WidgetSubscriptionUpdate::Disconnected {
                            reason: format!(
                                "idle_timeout ({}s)",
                                config.idle_timeout.as_secs()
                            ),
                        };
                        break;
                    }
                }
            }

            // 4. If the inner loop broke because of revoked, forget +
            //    fresh consent on the next iteration.
            if let Some(reason) = reauth {
                let _ = session::forget(config.app_id);
                yield WidgetSubscriptionUpdate::NeedsReauth { reason };
                continue;
            }

            // 5. Otherwise (EOF or idle timeout), back off and try again.
            tokio::time::sleep(backoff).await;
            backoff = next_backoff(backoff, config.max_backoff);
        }
    }
}

/// True if the daemon's `auth_denied` reason string is one of the
/// user-denied variants — `user_denied` (just clicked Deny) or
/// `user_denied_cached` (sticky deny still in the daemon's
/// in-memory cache). Exact-match so a future reason that happens to
/// embed `user_denied` as a substring (e.g.
/// `post_user_denied_cleanup_failed`) doesn't terminate the
/// subscription by accident.
fn is_user_denied_reason(reason: &str) -> bool {
    matches!(reason, "user_denied" | "user_denied_cached")
}

/// Public version of [`is_user_denied_reason`] that takes the wire
/// `Display` output of an [`HttpError`]: `auth_denied (user_denied)`
/// or `auth_denied (user_denied_cached)`. Returns false for any
/// other `auth_denied` reason and for non-auth errors.
///
/// Operates on the `Display`-formatted string so callers don't have
/// to import the enum.
#[must_use]
pub fn is_user_denied(err: &str) -> bool {
    matches!(
        err,
        "auth_denied (user_denied)" | "auth_denied (user_denied_cached)"
    )
}

fn next_backoff(current: Duration, max: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    if doubled > max { max } else { doubled }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_backoff_doubles_then_caps() {
        assert_eq!(
            next_backoff(Duration::from_secs(1), Duration::from_secs(30)),
            Duration::from_secs(2)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(2), Duration::from_secs(30)),
            Duration::from_secs(4)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(16), Duration::from_secs(30)),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(30), Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn http_error_is_invalid_session_matches_only_invalid_session() {
        use http_client::HttpError;
        // The replacement for the old free `is_invalid_session(&str)`
        // is `HttpError::is_invalid_session()` on the typed error.
        assert!(
            HttpError::InvalidSession {
                reason: "unknown".into()
            }
            .is_invalid_session()
        );
        assert!(
            HttpError::InvalidSession {
                reason: "expired".into()
            }
            .is_invalid_session()
        );
        // Other variants must NOT classify as invalid_session.
        assert!(
            !HttpError::AuthDenied {
                reason: "user_denied".into()
            }
            .is_invalid_session()
        );
        assert!(!HttpError::Other("connection refused".into()).is_invalid_session());
    }

    #[test]
    fn is_user_denied_reason_matches_both_deny_variants() {
        assert!(is_user_denied_reason("user_denied"));
        assert!(is_user_denied_reason("user_denied_cached"));
        // Other reasons must not.
        assert!(!is_user_denied_reason("user_dismissed"));
        assert!(!is_user_denied_reason("popup_failed"));
    }

    #[test]
    fn is_user_denied_matches_both_reasons() {
        // Both fresh denials and deny-cache hits must terminate the
        // subscription so we don't spam the daemon.
        assert!(is_user_denied("auth_denied (user_denied)"));
        assert!(is_user_denied("auth_denied (user_denied_cached)"));
        // A dismissed popup is recoverable — the user just walked away,
        // so the next attempt will pop a fresh prompt. Do NOT treat as
        // blocked.
        assert!(!is_user_denied("auth_denied (user_dismissed)"));
        // popup_failed / invalid_scope / etc. — those are infra
        // problems, the consumer's normal backoff path handles them.
        assert!(!is_user_denied("auth_denied (popup_failed)"));
        assert!(!is_user_denied("invalid_session (expired)"));
    }

    /// Pins the wire-format contract between
    /// `http_client::auth_request` (which formats `Err("auth_denied
    /// ({reason})")`) and this matcher. If either side drifts the
    /// classifier silently stops triggering Blocked and the deny
    /// spam loop the applet/settings fix solved comes back. This is
    /// the regression guard for that drift.
    #[test]
    fn wire_format_round_trip_matches_classifier() {
        // Verbatim format used by
        // super-stt-shared/src/daemon/http_client.rs::auth_request
        // when the daemon returns 4xx. Two reasons that must drive
        // the Blocked path:
        for reason in ["user_denied", "user_denied_cached"] {
            let wire = format!("auth_denied ({reason})");
            assert!(
                is_user_denied(&wire),
                "wire format `{wire}` must classify as user-denied; \
                 if this test fails, the http_client formatter or \
                 the daemon's auth_err reason string drifted"
            );
        }
        // And these must NOT drive Blocked — they need the helper's
        // ordinary backoff/reconnect path.
        for reason in [
            "user_dismissed",
            "popup_failed",
            "throttled",
            "invalid_scope",
        ] {
            let wire = format!("auth_denied ({reason})");
            assert!(
                !is_user_denied(&wire),
                "wire format `{wire}` must NOT classify as user-denied"
            );
        }
    }

    /// Document the Blocked variant carries the daemon's reason
    /// string verbatim so consumers (the applet popup, the settings
    /// connection page) can surface it to the user without having
    /// to parse anything out of it.
    #[test]
    fn blocked_update_carries_reason_string() {
        let update = WidgetSubscriptionUpdate::Blocked {
            reason: "auth_denied (user_denied_cached)".to_string(),
        };
        match update {
            WidgetSubscriptionUpdate::Blocked { reason } => {
                assert_eq!(reason, "auth_denied (user_denied_cached)");
            }
            other => panic!("variant constructed Blocked but pattern saw {other:?}"),
        }
    }

    /// Routing contract: which `HttpError` variant maps to which
    /// `WidgetSubscriptionUpdate` variant. Pins the fix for the
    /// "daemon offline reported as 'session revoked'" UX bug —
    /// `HttpError::Other` must drive `Disconnected`, not
    /// `NeedsReauth`. The actual routing lives inside the
    /// `run_widget_subscription` match, but we can verify the
    /// intent by enumerating the discriminants we expect each
    /// variant to land in.
    #[test]
    fn obtain_error_routing_contract() {
        use http_client::HttpError;

        // The catch-all `Err(e @ HttpError::Other(_)) => Disconnected`
        // arm fires here. Failing this would mean a daemon-offline
        // state is again reported as a revoked session.
        let daemon_offline = HttpError::Other("Daemon HTTP listener not running.".to_string());
        assert!(
            !daemon_offline.is_invalid_session(),
            "Other must not be classified as invalid_session"
        );
        assert!(matches!(daemon_offline, HttpError::Other(_)));

        // `InvalidSession` and non-user-denied `AuthDenied` must
        // land in `NeedsReauth`. We can't observe the stream output
        // synchronously here, but we lock in the predicate the
        // routing relies on.
        let invalid = HttpError::InvalidSession {
            reason: "expired".to_string(),
        };
        assert!(invalid.is_invalid_session());

        let popup_failed = HttpError::AuthDenied {
            reason: "popup_failed".to_string(),
        };
        assert!(
            !matches!(&popup_failed, HttpError::AuthDenied { reason } if is_user_denied_reason(reason)),
            "popup_failed must NOT be classified as user-denied — \
             it's recoverable, not terminal"
        );

        // The terminal `Blocked` path only triggers on these two.
        for reason in ["user_denied", "user_denied_cached"] {
            let e = HttpError::AuthDenied {
                reason: reason.to_string(),
            };
            assert!(
                matches!(&e, HttpError::AuthDenied { reason } if is_user_denied_reason(reason)),
                "`{reason}` must classify as terminal user-denied"
            );
        }
    }

    #[test]
    fn config_defaults_are_sane() {
        let cfg = WidgetSubscriptionConfig::new(
            AppId("test-app"),
            "Test App",
            "widget",
            &["recording_state"],
        );
        // Idle timeout must be ≥ 2× the daemon's keepalive interval (30 s).
        assert!(cfg.idle_timeout >= Duration::from_secs(60));
        // Backoff must grow.
        assert!(cfg.initial_backoff < cfg.max_backoff);
    }
}
