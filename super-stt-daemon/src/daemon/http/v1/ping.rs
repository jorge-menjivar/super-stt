// SPDX-License-Identifier: GPL-3.0-only
//! `/ping` — liveness.
//!
//! Contract: `docs/protocol/endpoints/v1/ping.md`.
//!
//! The cheapest thing a client can ask. What the daemon is actually *running*
//! is [`super::status`].

use crate::daemon::http::internal::helpers::dispatch::ack;
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{Ack, ErrorEnvelope, ReasonEnvelope};
use axum::extract::State;
use axum::response::Response;

#[utoipa::path(
    get,
    path = "/ping",
    tag = "health",
    summary = "Liveness probe",
    description = "\
Confirms the listener is reachable and the presented token is valid. Introspects \
no state.

To check whether the *token* is still good without risking a consent popup, prefer \
`GET /auth/status` — that is the dedicated probe.",
    security(("session_token" = [])),
    responses(
        (status = 200, description = "The daemon is up. `message` is always `pong`.", body = Ack,
         example = json!({ "status": "success", "message": "pong" })),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "Connection refused — the daemon is over its per-client connection cap.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn ping(State(s): State<AppState>) -> Response {
    ack(&s.daemon, "ping", None).await
}
