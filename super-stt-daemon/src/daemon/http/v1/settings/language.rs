// SPDX-License-Identifier: GPL-3.0-only
//! `/language` (global) transcription-language settings routes.
//!
//! The per-model override moved to
//! `/backends/{source}/models/{model}/language` (see
//! `crate::daemon::http::v1::backends::model_language`).
use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SetLanguageBody {
    pub(crate) language: String,
}

pub(crate) async fn get_language(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "get_primary_language", None).await
}

pub(crate) async fn set_language(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetLanguageBody>,
) -> impl IntoResponse {
    dispatch_command(
        &s.daemon,
        "set_primary_language",
        Some(serde_json::json!({ "language": body.language })),
    )
    .await
}

pub(crate) async fn clear_language(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "clear_primary_language", None).await
}
