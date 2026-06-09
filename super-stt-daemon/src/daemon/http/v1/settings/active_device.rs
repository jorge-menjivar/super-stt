// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SetActiveDeviceBody {
    pub(crate) device: String,
}

pub(crate) async fn set_active_device(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetActiveDeviceBody>,
) -> impl IntoResponse {
    dispatch_command(
        &s.daemon,
        "set_device",
        Some(serde_json::json!({ "device": body.device })),
    )
    .await
}

pub(crate) async fn get_active_device(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "get_device", None).await
}
