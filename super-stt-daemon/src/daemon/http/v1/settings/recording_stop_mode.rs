// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SetRecordingStopModeBody {
    pub(crate) mode: String,
}

pub(crate) async fn set_recording_stop_mode(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetRecordingStopModeBody>,
) -> impl IntoResponse {
    dispatch_command(
        &s.daemon,
        "set_recording_stop_mode",
        Some(serde_json::json!({ "mode": body.mode })),
    )
    .await
}

pub(crate) async fn get_recording_stop_mode(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "get_recording_stop_mode", None).await
}
