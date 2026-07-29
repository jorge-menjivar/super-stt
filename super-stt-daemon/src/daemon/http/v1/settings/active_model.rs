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
    #[serde(default)]
    pub(crate) source: Option<String>,
}

pub(crate) async fn set_active_model(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetActiveModelBody>,
) -> impl IntoResponse {
    let mut data = serde_json::json!({
        "model":    body.model,
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

    let body = active_model_body(
        model_resp.current_model.as_deref(),
        model_resp.current_source.as_deref(),
        model_resp.model_loaded.unwrap_or(false),
        device_resp.device.as_deref().unwrap_or("unknown"),
        switch_payload.as_ref(),
    );
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// Build the `GET /active_model` payload.
///
/// `current.provider` is a compatibility shim, same as the one on
/// `GET /registry/backends` (see [`IndexModel::provider`]): it is always an
/// empty string and carries no information — a model is identified by
/// `(name, source)` — but clients through v0.2.0 unwrap the key and error out
/// when it is absent, so the response would stop parsing for them entirely.
///
/// [`IndexModel::provider`]: super_stt_registry_types::index::IndexModel::provider
fn active_model_body(
    model: Option<&str>,
    source: Option<&str>,
    loaded: bool,
    device: &str,
    switch: Option<&serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "status": "success",
        "active_model": {
            "current": {
                "model":    model,
                "source":   source,
                "provider": "",
                "loaded":   loaded,
                "device":   device,
            },
            "switch": switch,
        }
    })
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

#[cfg(test)]
mod tests {
    use super::active_model_body;

    /// `GET /active_model` must keep carrying `current.provider`. Clients
    /// through v0.2.0 unwrap the key and return an error when it is absent, so
    /// dropping it does not degrade their Models page — it makes every
    /// `get_current_model()` fail, leaving the UI stuck on "no model loaded"
    /// while transcription works fine.
    ///
    /// This is the test that fails if the compatibility shim is deleted before
    /// those clients have rolled over.
    #[test]
    fn the_response_still_carries_the_provider_key() {
        let body = active_model_body(
            Some("voxtral-mini"),
            Some("github.com/super-stt/voxtral"),
            true,
            "cuda",
            None,
        );
        let current = &body["active_model"]["current"];
        assert!(
            current.get("provider").is_some(),
            "GET /active_model dropped `current.provider`; clients <= v0.2.0 error on this: {body}"
        );
        assert_eq!(
            current["provider"], "",
            "the shim carries no information — a model is (name, source)"
        );
    }

    /// The idle payload is the one a client hits before any model is loaded,
    /// so it has to satisfy the same clients. A shim emitted only on the
    /// loaded path would still break their first poll.
    #[test]
    fn the_idle_response_carries_it_too() {
        let body = active_model_body(None, None, false, "unknown", None);
        let current = &body["active_model"]["current"];
        assert!(
            current.get("provider").is_some(),
            "idle `GET /active_model` dropped `current.provider`: {body}"
        );
        assert!(
            current["model"].is_null(),
            "idle payload must null the model"
        );
    }
}
