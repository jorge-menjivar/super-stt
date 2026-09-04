// SPDX-License-Identifier: GPL-3.0-only
//! `GET /update` and `POST /update/check` — Super STT updating itself.
//!
//! Contract: `docs/protocol/endpoints/v1/update.md`. Distinct from the *backend*
//! update surface under `/registry`, which updates models rather than the daemon.

use axum::extract::State;
use axum::response::IntoResponse;

use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use super_stt_shared::models::self_update::SelfUpdateStatus;

#[utoipa::path(
    get,
    path = "/update",
    tag = "update",
    summary = "Read the last self-update check",
    description = "\
Reports what the most recent check found, without performing one. `checked_at` is \
`null` until a check has run, and `last_check_error` says why the last attempt \
failed if it did.

Which channel is consulted follows `/settings/update_beta_optin`; `beta_optin_effective` \
reports the setting even before a check has resolved a channel of its own. To force \
a check now, use `POST /update/check`.",
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The last known update state.", body = SelfUpdateStatus),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn get_update(State(s): State<AppState>) -> impl IntoResponse {
    // The configured opt-in, so `beta_optin_effective` is answered from the
    // setting before the first check has resolved a channel of its own.
    let optin = s.daemon.config.read().await.update.beta_optin;
    axum::Json(s.daemon.self_update.status_for(optin).await)
}

#[utoipa::path(
    post,
    path = "/update/check",
    tag = "update",
    summary = "Check for a Super STT update now",
    description = "\
Runs the check immediately rather than waiting for the daemon's own schedule, and \
answers with the result — the same shape `GET /update` reports. Works whether or not \
the periodic check is enabled.

This only *looks*. Nothing is downloaded or installed as a result.",
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The check ran; this is what it found. A network failure is reported in `last_check_error`, not as an HTTP error.", body = SelfUpdateStatus),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn post_check(State(s): State<AppState>) -> impl IntoResponse {
    axum::Json(s.daemon.run_self_update_check_and_notify().await)
}
