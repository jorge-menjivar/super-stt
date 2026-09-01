// SPDX-License-Identifier: GPL-3.0-only
//! `/models` — the models the active backend serves.
//!
//! Selecting and loading a model happens through `/pipeline/{stage}/model`
//! (see `super::pipeline`); this module is only the flat catalog read that a
//! picker fills itself from.
//!
//! The `current.provider` compatibility shim that `GET /active_model` carried
//! for clients through v0.2.0 went with that endpoint — those clients need the
//! pipeline paths now, so there is nothing left for the shim to protect.

use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::response::IntoResponse;

pub(crate) async fn list_models(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "list_models", None).await
}
