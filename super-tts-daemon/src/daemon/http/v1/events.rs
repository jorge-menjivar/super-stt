// SPDX-License-Identifier: GPL-3.0-only
//! `/events` — one Server-Sent Events stream carrying everything the daemon
//! publishes.
//!
//! Contract: `docs/protocol/endpoints/v1/events.md`.
//!
//! The route sits in the scope-less group because its gate is per *topic*, not
//! per endpoint: any valid token may open the connection, and the subscription
//! is then refused whole if the token is short a scope for any topic asked for.
//! Enforcing that here rather than in a router guard is what lets one endpoint
//! serve `playback_events`, `audio_visualization` and `daemon_status` at once.
//!
//! The stream itself is `super_engine_daemon::events::stream`, shared with
//! Super STT; what is Super TTS's is the topic list, documented here.

use crate::daemon::http::state::{AppState, PeerInfo};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use axum::extract::{Query, State};
use axum::response::Response;
use super_engine_daemon::auth::AuthContext;
use super_engine_daemon::events::EventsQuery;

#[utoipa::path(
    get,
    path = "/events",
    tag = "events",
    summary = "Subscribe to the event stream",
    description = "\
A Server-Sent Events stream of everything the daemon publishes. The response is \
`text/event-stream` and stays open until the client disconnects, the daemon shuts \
down, or the calling binary changes on disk (which emits a final `revoked` frame \
and invalidates the token).

The first frame is always `subscribed`, carrying a `client_id` and the topic list \
that was accepted. A `: keepalive` comment is sent periodically so idle connections \
are not reaped by intermediaries. Subsequent frames set `event:` to the topic name \
and `data:` to that topic's payload, with no outer envelope.

**Scopes are per topic, not per endpoint.** Any valid token reaches this route; the \
subscription is then refused whole if the token lacks the scope for *any* requested \
topic, so ask only for what you hold.

| Topic | Scope | Payload |
|---|---|---|
| `speaking_state` | `playback_events` | `{ is_speaking, utterance_id? }` |
| `speech_progress` | `playback_events` | `{ utterance_id, spoken_ms, queued_ms }` |
| `frequency_bands` | `audio_visualization` | `{ bands_b64, sample_rate, total_energy }` |
| `daemon_status_changed`, `download_progress`, `registry_install` | `daemon_status` | see `docs/protocol/endpoints/v1/events.md` |

This is how a client follows an utterance past the request that started it: \
`POST /speak` returns as soon as the audio is queued, and `speaking_state` / \
`speech_progress` outlive the connection that asked for it.

Under backpressure the daemon drops frames rather than buffering them without \
bound; `frequency_bands` is what sheds in practice, since it is emitted many times \
per second and the control frames are not.",
    params(
        ("topics" = String, Query,
         description = "Comma-separated topic names, at least one. Unknown or empty → `400 invalid_topic`.",
         example = "speaking_state,speech_progress"),
    ),
    security(("session_token" = [])),
    responses(
        (status = 200, description = "The stream is open. Frames follow as `event:`/`data:` pairs.",
         content_type = "text/event-stream"),
        (status = 400, description = "`topics` was missing, empty, or named a topic that does not exist.", body = ReasonEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the scope for one of the requested topics.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
/// `GET /events?topics=...` — widget SSE subscription. See
/// `super_engine_daemon::events::stream`.
pub(crate) async fn events(
    State(s): State<AppState>,
    Query(q): Query<EventsQuery>,
    ctx: Option<axum::Extension<AuthContext>>,
    peer: Option<axum::Extension<PeerInfo>>,
) -> Response {
    super_engine_daemon::events::stream(
        s.daemon.events.as_ref(),
        q.topics.as_deref(),
        ctx.map(|axum::Extension(ctx)| ctx),
        peer.as_ref().map(|axum::Extension(peer)| peer),
        s.auth.tokens(),
    )
}

/// Server-Sent Events subscription route.
pub(crate) fn routes() -> utoipa_axum::router::OpenApiRouter<AppState> {
    utoipa_axum::router::OpenApiRouter::new().routes(utoipa_axum::routes!(events))
}
