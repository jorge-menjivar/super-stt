// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, json_response};
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::response::IntoResponse;

pub(crate) async fn ping(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("ping", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

pub(crate) async fn status(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("status", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}
