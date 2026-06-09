// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SetVolumeBody {
    pub(crate) volume: u8,
}

pub(crate) async fn set_volume(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetVolumeBody>,
) -> impl IntoResponse {
    dispatch_command(
        &s.daemon,
        "set_volume",
        Some(serde_json::json!({ "volume": body.volume })),
    )
    .await
}

pub(crate) async fn get_volume(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "get_volume", None).await
}
