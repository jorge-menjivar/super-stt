// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::auth::middleware::AuthContext;
use crate::daemon::http::internal::auth::tokens::TokenStore;
use crate::daemon::http::internal::helpers::responses::{invalid_session, reason, scope_denied};
use crate::daemon::http::state::{AppState, PeerInfo};
use crate::daemon::http::v1::transcribe::format_sse_frame;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::path::PathBuf;

// ---------- /events (SSE) --------------------------------------------------

/// Per-connection SSE channel capacity. The channel serializes every frame
/// (all topic forwarders + keepalive + revocation) into the response body. A
/// widget that stops draining its side would otherwise buffer `frequency_bands`
/// frames — emitted many times per second — without bound; capping the channel
/// and dropping frames once it fills sheds that load instead (Tier 3 #8). The
/// dominant volume is visualization frames, so those are what overflow drops in
/// practice; low-rate control frames only shed for an already-stalled reader,
/// which the keepalive / exe-watch task then tears down.
const SSE_CHANNEL_CAPACITY: usize = 256;

/// Convenience alias for the bounded per-connection SSE sender.
type SseSender = tokio::sync::mpsc::Sender<Result<axum::body::Bytes, std::io::Error>>;

/// Try to enqueue an SSE frame on the bounded per-connection channel. Returns
/// `false` only when the receiver is gone (client disconnected), so the caller
/// tears down. A full channel drops the frame — the reader is stalled, so shed
/// it — and returns `true`.
fn try_emit_sse_event(tx: &SseSender, event: &str, data: &serde_json::Value) -> bool {
    use tokio::sync::mpsc::error::TrySendError;
    match tx.try_send(Ok(format_sse_frame(event, data))) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            log::warn!("widget SSE backpressured; dropped a {event} frame");
            true
        }
        Err(TrySendError::Closed(_)) => false,
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct EventsQuery {
    /// Comma-separated topic names. Empty / missing → 400 `invalid_topic`.
    topics: Option<String>,
}

fn invalid_topic(reason: &str) -> Response {
    let body = serde_json::json!({
        "status":  "error",
        "message": "invalid_topic",
        "data":    { "reason": reason },
    });
    (
        StatusCode::BAD_REQUEST,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// `GET /events?topics=...` — widget SSE subscription.
///
/// The handler runs forever (until the client disconnects, the daemon
/// shuts down, or `exe_changed` triggers a revoked event). Each
/// requested topic gets a per-connection `broadcast::Receiver` which
/// runs in its own forwarder task — so all subscribers receive events
/// independently and a slow widget never starves a fast one (and vice
/// versa).
///
/// The forwarder tasks share a `CancellationToken` with the keepalive
/// + exe-watch task, so any of `client disconnect / exe_changed /
/// shutdown` cleanly tears the whole subscription down.
pub(crate) async fn events(
    State(s): State<AppState>,
    Query(q): Query<EventsQuery>,
    ctx: Option<axum::Extension<AuthContext>>,
    peer: Option<axum::Extension<PeerInfo>>,
) -> Response {
    let requested = match parse_events_topics(&q) {
        Ok(t) => t,
        Err(reason) => return invalid_topic(&reason),
    };

    // Auth context — should always be present after middleware ran, but
    // we degrade gracefully if it isn't (treat as missing session).
    let Some(axum::Extension(ctx)) = ctx else {
        return invalid_session(reason::UNKNOWN);
    };

    // Each requested topic is gated by the scope that grants it. If the
    // token is missing the scope for any requested topic, the whole
    // subscription is refused before it opens.
    if requested
        .iter()
        .any(|t| !ctx.meta.scopes.iter().any(|s| s == t.required_scope()))
    {
        return scope_denied();
    }

    // Bounded mpsc that serializes all SSE writes (broadcast forwarders +
    // keepalive + revocation). Bounded so a stalled reader sheds frames instead
    // of buffering without limit — see [`SSE_CHANNEL_CAPACITY`].
    let (sse_tx, sse_rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(
        SSE_CHANNEL_CAPACITY,
    );

    // Subscribe BEFORE emitting `subscribed`. Subscribing creates the
    // broadcast::Receiver that captures any event fired from this
    // point on — and once we send the `subscribed` ack the client is
    // entitled to assume every subsequent event reaches it. Doing the
    // ack first would leave a gap where events fire and are missed
    // even though the client believes the subscription is live.
    let topic_names: Vec<&'static str> = requested.iter().map(|t| t.as_str()).collect();
    let cancel = tokio_util::sync::CancellationToken::new();
    for topic in &requested {
        let rx = s.daemon.events.subscribe(*topic);
        spawn_topic_forwarder(rx, sse_tx.clone(), cancel.clone());
    }

    // Now that the receivers exist, ack the client. The channel is fresh so
    // this frame always has room.
    let _ = try_emit_sse_event(
        &sse_tx,
        "subscribed",
        &serde_json::json!({
            "client_id": uuid::Uuid::new_v4().to_string(),
            "subscribed_to": topic_names,
        }),
    );

    spawn_events_keepalive_and_exe_watch(
        sse_tx.clone(),
        cancel,
        peer.and_then(|p| p.0.pid),
        s.tokens.clone(),
        ctx.token,
        ctx.meta.exe_path,
    );

    // The handler's own `sse_tx` clone is dropped here. The forwarders
    // and the timer task own the remaining clones; once they all
    // finish, the mpsc receiver yields None and the response body ends.
    drop(sse_tx);

    let stream = tokio_stream::wrappers::ReceiverStream::new(sse_rx);
    let body = axum::body::Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-store")
        .header("x-accel-buffering", "no")
        .body(body)
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("content-type", "application/json")],
                String::from("{\"status\":\"error\"}"),
            )
                .into_response()
        })
}

/// Parse the `?topics=` query string into a deduplicated `Vec<Topic>`.
/// Returns the raw bad-topic name (or `"missing_topics"` for missing /
/// empty queries) on the `Err` arm so the caller can produce the
/// matching `400 invalid_topic` response. Boxing the response would be
/// fine but using a small string keeps the error type cheap to clone /
/// move.
fn parse_events_topics(q: &EventsQuery) -> Result<Vec<crate::daemon::events::Topic>, String> {
    use crate::daemon::events::Topic;

    let csv = match q.topics.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => return Err("missing_topics".to_string()),
    };
    let mut requested: Vec<Topic> = Vec::new();
    for raw in csv.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        match Topic::from_wire(raw) {
            Some(t) if !requested.contains(&t) => requested.push(t),
            Some(_) => {} // duplicate
            None => return Err(raw.to_string()),
        }
    }
    if requested.is_empty() {
        return Err("missing_topics".to_string());
    }
    Ok(requested)
}

/// Spawn a per-topic forwarder. Reads from the broadcast receiver and
/// writes each event as an SSE frame. Exits on cancel, on a closed
/// channel, or when the SSE response body has been dropped.
fn spawn_topic_forwarder(
    mut rx: crate::daemon::events::AnyReceiver,
    tx: SseSender,
    cancel: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                res = rx.recv_json() => {
                    match res {
                        Ok((name, payload)) => {
                            if !try_emit_sse_event(&tx, name, &payload) {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            log::warn!("widget SSE lagged: dropped {n} events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });
}

/// Spawn the timer task that drives keep-alive comments and the
/// periodic exe-path check. On `exe_changed` the task emits a
/// `revoked` event, calls `TokenStore::revoke`, and triggers the
/// shared `cancel` token to tear down the rest of the subscription.
fn spawn_events_keepalive_and_exe_watch(
    tx: SseSender,
    cancel: tokio_util::sync::CancellationToken,
    peer_pid: Option<u32>,
    tokens: TokenStore,
    token_str: String,
    stored_exe: PathBuf,
) {
    use tokio::time::{Duration, MissedTickBehavior, interval};

    tokio::spawn(async move {
        // Both timers are 30 s (cheap), aligned by `MissedTickBehavior::Skip`
        // so a temporarily-blocked task doesn't accumulate stale ticks.
        let mut keepalive = interval(Duration::from_secs(30));
        keepalive.set_missed_tick_behavior(MissedTickBehavior::Skip);
        keepalive.tick().await; // immediate first tick — discard
        let mut exe_watch = interval(Duration::from_secs(30));
        exe_watch.set_missed_tick_behavior(MissedTickBehavior::Skip);
        exe_watch.tick().await;

        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                _ = keepalive.tick() => {
                    // Only a gone receiver (client disconnected) tears down; a
                    // full channel means a stalled-but-live reader, so drop this
                    // heartbeat rather than cancel.
                    use tokio::sync::mpsc::error::TrySendError;
                    if let Err(TrySendError::Closed(_)) =
                        tx.try_send(Ok(axum::body::Bytes::from_static(b": keepalive\n\n")))
                    {
                        cancel.cancel();
                        break;
                    }
                }
                _ = exe_watch.tick() => {
                    let Some(pid) = peer_pid else { continue; };
                    let current = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
                    if current.as_ref().is_some_and(|c| *c == stored_exe) {
                        continue;
                    }
                    log::info!(
                        "widget exe_changed on pid {pid}: stored={} current={:?}; revoking session",
                        stored_exe.display(),
                        current,
                    );
                    let _ = try_emit_sse_event(
                        &tx,
                        "revoked",
                        &serde_json::json!({ "reason": "exe_changed" }),
                    );
                    tokens.revoke(&token_str);
                    cancel.cancel();
                    break;
                }
            }
        }
    });
}

/// Server-Sent Events subscription route.
pub(crate) fn routes() -> axum::Router<crate::daemon::http::state::AppState> {
    axum::Router::new().route("/events", axum::routing::get(events))
}
