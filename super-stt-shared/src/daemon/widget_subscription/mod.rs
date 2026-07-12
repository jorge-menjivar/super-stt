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
use std::path::{Path, PathBuf};
use std::pin::Pin;
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
/// only the `app_id`/`app_name`/`scopes`/`topics` fields differing.
#[derive(Clone, Copy)]
pub struct WidgetSubscriptionConfig {
    /// Stable per-app keyring user (e.g. `"super-stt-cosmic-applet"`).
    pub app_id: AppId,
    /// Human-readable name shown in the consent popup.
    pub app_name: &'static str,
    /// Scopes requested at consent time. Must grant every topic in
    /// `topics` (each topic is gated by its scope; see the scope docs).
    pub scopes: &'static [&'static str],
    /// Topics to subscribe to. Every topic must be granted by `scopes`,
    /// or the daemon refuses the whole subscription with `403 scope_denied`.
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
        scopes: &'static [&'static str],
        topics: &'static [&'static str],
    ) -> Self {
        debug_assert!(
            uncovered_topic(scopes, topics).is_none(),
            "widget subscription requests a topic its scopes don't grant: {:?}",
            uncovered_topic(scopes, topics),
        );
        Self {
            app_id,
            app_name,
            scopes,
            topics,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
        }
    }
}

/// Canonical `topic → required scope` mapping for the daemon's
/// `GET /events` stream — the single source of truth shared by every
/// widget client. Mirrors the daemon's `Topic::required_scope` (a
/// daemon-side test pins the two together) and the topic tables in
/// `docs/protocol/endpoints/v1/events.md`. Returns `None` for an
/// unknown topic name.
#[must_use]
pub fn required_scope_for_topic(topic: &str) -> Option<&'static str> {
    Some(match topic {
        "recording_started"
        | "recording_stopped"
        | "recording_state"
        | "transcribing_started"
        | "transcribing_stopped" => "recording_events",
        "frequency_bands" => "audio_visualization",
        "partial_stt" | "final_stt" => "global_transcriptions",
        "daemon_status_changed" | "download_progress" | "registry_install" => "daemon_status",
        _ => return None,
    })
}

/// The first topic in `topics` that `scopes` does not grant (or whose
/// name is unknown), or `None` when every topic is covered. `None`
/// means the daemon will not refuse the subscription with
/// `403 scope_denied` for a missing-scope reason. Clients assert this in
/// their tests so a topic added without its scope fails CI rather than
/// silently 403-ing the whole stream at runtime.
#[must_use]
pub fn uncovered_topic<'t>(scopes: &[&str], topics: &[&'t str]) -> Option<&'t str> {
    topics
        .iter()
        .copied()
        .find(|t| match required_scope_for_topic(t) {
            Some(required) => !scopes.contains(&required),
            None => true,
        })
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

/// True when `err` is an [`HttpError::AuthDenied`] the *user* triggered
/// (`user_denied` / `user_denied_cached`) — the only auth failures that should
/// suppress the reconnect loop. Any other `auth_denied` reason (e.g.
/// `user_dismissed`, `popup_failed`) and every non-auth error return false.
/// Matches the typed variant, not the error's wording.
#[must_use]
pub fn is_user_denied(err: &crate::daemon::http_client::HttpError) -> bool {
    matches!(err, crate::daemon::http_client::HttpError::AuthDenied { reason } if is_user_denied_reason(reason))
}

fn next_backoff(current: Duration, max: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    if doubled > max { max } else { doubled }
}

/// Outcome of trying to obtain a token at the top of a loop iteration.
enum TokenOutcome {
    Ready(String),
    /// Terminal: user denied. Yield Blocked and end the stream.
    Blocked(String),
    /// Recoverable auth failure: yield `NeedsReauth`, back off, retry.
    Reauth(String),
    /// Transport/other failure: yield Disconnected, back off, retry.
    Disconnected(String),
}

async fn acquire_token(socket: &Path, config: &WidgetSubscriptionConfig) -> TokenOutcome {
    match session::obtain(
        socket.to_path_buf(),
        config.app_id,
        config.app_name,
        config.scopes,
    )
    .await
    {
        Ok(t) => TokenOutcome::Ready(t),
        Err(http_client::HttpError::AuthDenied { reason }) if is_user_denied_reason(&reason) => {
            TokenOutcome::Blocked(format!("auth_denied ({reason})"))
        }
        Err(
            e @ (http_client::HttpError::AuthDenied { .. }
            | http_client::HttpError::InvalidSession { .. }),
        ) => TokenOutcome::Reauth(e.to_string()),
        Err(e @ http_client::HttpError::Other(_)) => TokenOutcome::Disconnected(e.to_string()),
    }
}

/// Outcome of opening the SSE stream.
enum OpenOutcome {
    Ready(Pin<Box<dyn futures_util::Stream<Item = WidgetEvent> + Send>>),
    /// Cached token rejected — forget already called; yield `NeedsReauth`,
    /// retry immediately (no backoff).
    Reauth(String),
    /// Transport/connection failure; yield `Disconnected`, back off, retry.
    Disconnected(String),
}

async fn open_stream(socket: &Path, token: &str, config: &WidgetSubscriptionConfig) -> OpenOutcome {
    match http_client::events_stream(socket.to_path_buf(), token, config.topics).await {
        Ok(s) => OpenOutcome::Ready(Box::pin(s)),
        Err(e) if e.is_invalid_session() => {
            let _ = session::forget(config.app_id);
            OpenOutcome::Reauth(e.to_string())
        }
        Err(e) => OpenOutcome::Disconnected(e.to_string()),
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
pub fn run_widget_subscription(
    socket: PathBuf,
    config: WidgetSubscriptionConfig,
) -> impl futures_util::Stream<Item = WidgetSubscriptionUpdate> + Send + 'static {
    async_stream::stream! {
        let mut backoff = config.initial_backoff;
        loop {
            let token = match acquire_token(&socket, &config).await {
                TokenOutcome::Ready(t) => t,
                TokenOutcome::Blocked(reason) => {
                    yield WidgetSubscriptionUpdate::Blocked { reason };
                    return;
                }
                TokenOutcome::Reauth(reason) => {
                    yield WidgetSubscriptionUpdate::NeedsReauth { reason };
                    tokio::time::sleep(backoff).await;
                    backoff = next_backoff(backoff, config.max_backoff);
                    continue;
                }
                TokenOutcome::Disconnected(reason) => {
                    yield WidgetSubscriptionUpdate::Disconnected { reason };
                    tokio::time::sleep(backoff).await;
                    backoff = next_backoff(backoff, config.max_backoff);
                    continue;
                }
            };

            let mut stream = match open_stream(&socket, &token, &config).await {
                OpenOutcome::Ready(s) => s,
                OpenOutcome::Reauth(reason) => {
                    yield WidgetSubscriptionUpdate::NeedsReauth { reason };
                    continue;
                }
                OpenOutcome::Disconnected(reason) => {
                    yield WidgetSubscriptionUpdate::Disconnected { reason };
                    tokio::time::sleep(backoff).await;
                    backoff = next_backoff(backoff, config.max_backoff);
                    continue;
                }
            };
            yield WidgetSubscriptionUpdate::Connected;
            backoff = config.initial_backoff;

            // Read with an idle deadline. EOF / timeout / revoked all break out.
            let mut reauth = None;
            loop {
                match tokio::time::timeout(config.idle_timeout, stream.next()).await {
                    Ok(Some(evt)) => {
                        if evt.name == "revoked" {
                            let reason = evt.payload.get("reason")
                                .and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                            reauth = Some(format!("revoked: {reason}"));
                            break;
                        }
                        if matches!(evt.name.as_str(), "subscribed" | "keepalive") {
                            continue;
                        }
                        yield WidgetSubscriptionUpdate::Event(evt);
                    }
                    Ok(None) => {
                        yield WidgetSubscriptionUpdate::Disconnected { reason: "stream ended".to_string() };
                        break;
                    }
                    Err(_) => {
                        yield WidgetSubscriptionUpdate::Disconnected {
                            reason: format!("idle_timeout ({}s)", config.idle_timeout.as_secs()),
                        };
                        break;
                    }
                }
            }

            if let Some(reason) = reauth {
                let _ = session::forget(config.app_id);
                yield WidgetSubscriptionUpdate::NeedsReauth { reason };
                continue;
            }
            tokio::time::sleep(backoff).await;
            backoff = next_backoff(backoff, config.max_backoff);
        }
    }
}

#[cfg(test)]
mod tests;
