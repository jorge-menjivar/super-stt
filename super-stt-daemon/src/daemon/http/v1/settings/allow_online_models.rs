// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, json_response};
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct AllowOnlineModelsBody {
    pub(crate) enabled: bool,
}

pub(crate) async fn set_allow_online_models(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<AllowOnlineModelsBody>,
) -> impl IntoResponse {
    let mut req = build_request("set_allow_online_models", None);
    req.enabled = Some(body.enabled);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

pub(crate) async fn get_allow_online_models(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("get_allow_online_models", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}
