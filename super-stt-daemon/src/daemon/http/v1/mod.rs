// SPDX-License-Identifier: GPL-3.0-only
pub(crate) mod auth;
pub(crate) mod events;
pub(crate) mod health;
pub(crate) mod registry;
pub(crate) mod settings;
pub(crate) mod transcribe;

use crate::daemon::http::internal::auth::middleware::{
    require_any_authenticated, require_rate_limit, require_settings_scope, require_status_scope,
    require_transcribe_scope,
};
use crate::daemon::http::state::AppState;
use axum::Router;
use axum::middleware;
use axum::routing::{get, post};

/// Assemble the `/v1` router with scope-aware middleware and state.
///
/// Endpoint groups split by required scope:
/// - `/auth/request`: reachable WITHOUT a token (it's how you get one).
/// - any-authenticated: `/ping`, `/auth/status`, and `GET /events` (the
///   per-topic scope is enforced inside the events handler).
/// - `status` scope: `GET /status`.
/// - `transcribe` scope: the transcription routes.
/// - `settings` scope: the configuration + registry surface.
///
/// All routes are bare (`/ping`, …); the `/v1` prefix is applied via `nest`.
pub(crate) fn router(state: AppState) -> Router {
    let any_scope = Router::new()
        .route("/ping", get(health::ping))
        .route("/auth/status", get(auth::status::auth_status))
        .merge(events::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_rate_limit,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_any_authenticated,
        ));

    let status_scope = Router::new()
        .route("/status", get(health::status))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_rate_limit,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_status_scope,
        ));

    let transcribe_scope = transcribe::routes()
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_rate_limit,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_transcribe_scope,
        ));

    let settings_scope = settings::routes()
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_rate_limit,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_settings_scope,
        ));

    let v1 = Router::new()
        .merge(any_scope)
        .merge(status_scope)
        .merge(transcribe_scope)
        .merge(settings_scope)
        .route("/auth/request", post(auth::request::auth_request));

    Router::new().nest("/v1", v1).with_state(state)
}
