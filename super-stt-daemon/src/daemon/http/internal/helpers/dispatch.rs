// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperSTTDaemon;
use axum::http::StatusCode;
use serde_json::Value;
use super_stt_shared::models::protocol::{DaemonRequest, DaemonResponse};

pub(crate) async fn dispatch(daemon: &SuperSTTDaemon, request: DaemonRequest) -> DaemonResponse {
    daemon.handle_command(request).await
}

pub(crate) fn build_request(command: &str, data: Option<Value>) -> DaemonRequest {
    DaemonRequest {
        command: command.to_string(),
        audio_data: None,
        sample_rate: None,
        client_id: Some(format!("http-cli-{}", uuid::Uuid::new_v4())),
        event_types: None,
        client_info: None,
        since_timestamp: None,
        limit: None,
        event_type: None,
        data,
        language: None,
        enabled: None,
    }
}

/// Map a [`DaemonResponse`] to the HTTP status code it should surface
/// on the wire.
///
/// `DaemonResponse.message` is supposed to be a stable identifier
/// (per `docs/protocol/transport.md`), but in practice handlers
/// return free-form sentences here. Rather than refactor every
/// handler, this function matches on the substrings each error
/// message reliably contains and maps them to the right HTTP class:
///
/// | Category                                | HTTP |
/// |-----------------------------------------|------|
/// | "success" body                          | 200  |
/// | Validation failures (unknown enum/name) | 400  |
/// | Online-models gate or CUDA unavailable  | 400  |
/// | State conflicts (no switch, recording)  | 409  |
/// | Everything else                         | 500  |
///
/// Free-form text is fine in `message` — the matcher is conservative
/// (substring-on-stable-phrasing) and falls through to 500 for
/// anything unrecognized, which is the right default for unexpected
/// errors.
/// Phrases (matched as substrings) that map an error `message` to
/// `400 Bad Request`. These are bad-input conditions — the client
/// gave the daemon something it couldn't use.
const BAD_REQUEST_PHRASES: &[&str] = &[
    "Unknown model",
    "Unknown ",
    "CUDA unavailable",
    "Online models are disabled",
    "not a valid",
    "Invalid ",
    // `POST /active_model` `invalid_model` and `POST /active_backend`
    // `invalid_backend` — the client named a model/source no installed
    // backend serves (docs/protocol/endpoints/v1/{active_model,active_backend}.md).
    "No installed backend",
    // `POST /active_model/language` when the tag is not supported by (or the
    // model is not) multilingual (docs/protocol/endpoints/v1/active_model/language.md).
    "unsupported_language",
];

/// Phrases (matched as substrings) that map an error `message` to
/// `409 Conflict`. These are state-conflict conditions — the request
/// is well-formed but the daemon's current state forbids it.
///
/// Update this list when adding a new state-conflict error string;
/// the canonical strings live in `daemon/device_management.rs`,
/// `daemon/model_management.rs`, and `download_progress.rs`. Any
/// "Already using …" message should be a `DaemonResponse::success()`
/// (so it short-circuits to 200 above) — keep it that way and do
/// not add an alternate match here.
const CONFLICT_PHRASES: &[&str] = &[
    "No download in progress",
    "Cannot switch models during",
    "Cannot switch devices during",
    "Cannot switch devices when",
    "recording in progress",
    "A download is already in progress",
    "Another download is in progress",
    "Failed to register download",
    // Per-model language endpoints when no model is active (409 not_ready).
    "not_ready",
];

pub(crate) fn status_code_for_response(resp: &DaemonResponse) -> StatusCode {
    if resp.status == "success" {
        return StatusCode::OK;
    }
    let msg = resp.message.as_deref().unwrap_or("");

    if BAD_REQUEST_PHRASES.iter().any(|p| msg.contains(p)) {
        return StatusCode::BAD_REQUEST;
    }
    if CONFLICT_PHRASES.iter().any(|p| msg.contains(p)) {
        return StatusCode::CONFLICT;
    }
    StatusCode::INTERNAL_SERVER_ERROR
}

pub(crate) fn json_response(
    resp: &DaemonResponse,
) -> (StatusCode, [(&'static str, &'static str); 1], String) {
    let status = status_code_for_response(resp);
    let body =
        serde_json::to_string(&resp).unwrap_or_else(|_| String::from("{\"status\":\"error\"}"));
    (status, [("content-type", "application/json")], body)
}

/// Build a [`DaemonRequest`] for `command` with optional `data`, dispatch it,
/// and shape the [`DaemonResponse`] into the standard HTTP response.
pub(crate) async fn dispatch_command(
    daemon: &SuperSTTDaemon,
    command: &str,
    data: Option<Value>,
) -> (StatusCode, [(&'static str, &'static str); 1], String) {
    let resp = dispatch(daemon, build_request(command, data)).await;
    json_response(&resp)
}

#[cfg(test)]
mod tests {
    use super::status_code_for_response;
    use axum::http::StatusCode;
    use super_stt_shared::models::protocol::DaemonResponse;

    /// A request naming a model/backend that no installed backend serves is a
    /// client error, documented as `400 invalid_model` / `400 invalid_backend`
    /// (docs/protocol/endpoints/v1/{active_model,active_backend}.md) — not 500.
    #[test]
    fn no_installed_backend_messages_are_bad_request() {
        let unknown_model = DaemonResponse::error(
            "No installed backend serves whisper-tiny via local_whisper. \
             Install the backend or check the model name.",
        );
        assert_eq!(
            status_code_for_response(&unknown_model),
            StatusCode::BAD_REQUEST
        );

        let unknown_backend = DaemonResponse::error(
            "No installed backend with source github.com/x/y (or its files are missing)",
        );
        assert_eq!(
            status_code_for_response(&unknown_backend),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn state_conflict_maps_to_conflict() {
        let resp = DaemonResponse::error("No download in progress");
        assert_eq!(status_code_for_response(&resp), StatusCode::CONFLICT);
    }

    #[test]
    fn unclassified_error_stays_internal() {
        let resp = DaemonResponse::error("something unexpected blew up");
        assert_eq!(
            status_code_for_response(&resp),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn success_is_ok() {
        assert_eq!(
            status_code_for_response(&DaemonResponse::success()),
            StatusCode::OK
        );
    }
}
