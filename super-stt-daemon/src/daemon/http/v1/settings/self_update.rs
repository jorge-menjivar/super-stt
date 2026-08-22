// SPDX-License-Identifier: GPL-3.0-only
//! GET /v1/update and POST /v1/update/check.
//! Contract: docs/protocol/endpoints/v1/update.md

use axum::extract::State;
use axum::response::IntoResponse;

use crate::daemon::http::state::AppState;

pub(crate) async fn get_update(State(s): State<AppState>) -> impl IntoResponse {
    axum::Json(s.daemon.self_update.status().await)
}

pub(crate) async fn post_check(State(s): State<AppState>) -> impl IntoResponse {
    axum::Json(s.daemon.run_self_update_check_and_notify().await)
}
