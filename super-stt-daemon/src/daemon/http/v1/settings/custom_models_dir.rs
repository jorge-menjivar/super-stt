// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct CustomModelsDirBody {
    pub(crate) path: Option<String>,
}

pub(crate) async fn set_custom_models_dir(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<CustomModelsDirBody>,
) -> impl IntoResponse {
    dispatch_command(
        &s.daemon,
        "set_custom_models_dir",
        Some(serde_json::json!({ "path": body.path })),
    )
    .await
}

pub(crate) async fn get_custom_models_dir(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "get_custom_models_dir", None).await
}
