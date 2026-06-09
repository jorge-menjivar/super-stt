// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::helpers::dispatch::{
    build_request, dispatch, dispatch_command, json_response,
};
use crate::daemon::http::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SetActiveModelBody {
    pub(crate) model: String,
    pub(crate) provider: String,
    #[serde(default)]
    pub(crate) source: Option<String>,
}

pub(crate) async fn set_active_model(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetActiveModelBody>,
) -> impl IntoResponse {
    let mut data = serde_json::json!({
        "model":    body.model,
        "provider": body.provider,
    });
    if let Some(source) = body.source {
        data["source"] = serde_json::Value::String(source);
    }
    let req = build_request("set_model", Some(data));
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

pub(crate) async fn get_active_model(State(s): State<AppState>) -> impl IntoResponse {
    // Compose the legacy `get_model` + `get_device` + `get_download_status`
    // results into the doc-spec `{ active_model: { current, switch } }` shape.
    let model_resp = dispatch(&s.daemon, build_request("get_model", None)).await;
    let device_resp = dispatch(&s.daemon, build_request("get_device", None)).await;
    let download_resp = dispatch(&s.daemon, build_request("get_download_status", None)).await;

    // Device/download failures are genuine errors and surface upward. The
    // model dispatch, however, returns an error envelope in the normal idle
    // state (no model loaded) — that is NOT a failure: we still return a
    // well-formed `active_model` with a null `current.model` so clients can
    // render "no model selected" instead of choking on a missing field.
    for resp in [&device_resp, &download_resp] {
        if resp.status != "success" {
            return json_response(resp).into_response();
        }
    }

    let switch_payload = download_resp.download_progress.map(|p| {
        serde_json::json!({
            "phase":            p.status,
            "target":           { "model": p.model_name },
            "started_at":       p.started_at,
            "download": {
                "current_file":     p.current_file,
                "file_index":       p.file_index,
                "total_files":      p.total_files,
                "bytes_downloaded": p.bytes_downloaded,
                "total_bytes":      p.total_bytes,
                "percentage":       p.percentage,
                "eta_seconds":      p.eta_seconds,
            },
        })
    });

    let body = serde_json::json!({
        "status": "success",
        "active_model": {
            "current": {
                "model":    model_resp.current_model,
                "provider": model_resp.current_provider,
                "source":   model_resp.current_source,
                "loaded":   model_resp.model_loaded.unwrap_or(false),
                "device":   device_resp.device.unwrap_or_else(|| "unknown".to_string()),
            },
            "switch": switch_payload,
        }
    });
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

pub(crate) async fn cancel_set_active_model(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "cancel_download", None).await
}

pub(crate) async fn reload_active_model(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "reload_active_model", None).await
}

/// `DELETE /active_model` — drop the loaded model without changing the
/// active backend. The user can pick another model from the same backend
/// and load it. To return to fully idle, `DELETE /active_backend` instead.
pub(crate) async fn unload_active_model(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "unload_active_model", None).await
}

pub(crate) async fn list_models(State(s): State<AppState>) -> impl IntoResponse {
    dispatch_command(&s.daemon, "list_models", None).await
}
