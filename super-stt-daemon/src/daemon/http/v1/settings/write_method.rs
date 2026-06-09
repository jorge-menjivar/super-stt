// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SetWriteMethodBody {
    pub(crate) method: String,
}

pub(crate) async fn set_write_method(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetWriteMethodBody>,
) -> impl IntoResponse {
    dispatch_command(
        &s.daemon,
        "set_write_method",
        Some(serde_json::json!({ "method": body.method })),
    )
    .await
}

pub(crate) async fn get_write_method(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "get_write_method", None).await
}
