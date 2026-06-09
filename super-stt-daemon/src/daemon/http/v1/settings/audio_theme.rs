// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SetAudioThemeBody {
    pub(crate) theme: String,
}

pub(crate) async fn set_audio_theme(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetAudioThemeBody>,
) -> impl IntoResponse {
    dispatch_command(
        &s.daemon,
        "set_audio_theme",
        Some(serde_json::json!({ "theme": body.theme })),
    )
    .await
}

pub(crate) async fn get_audio_theme(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "get_audio_theme", None).await
}

pub(crate) async fn test_audio_theme(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "test_audio_theme", None).await
}

pub(crate) async fn list_audio_themes(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "list_audio_themes", None).await
}
