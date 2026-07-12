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

/// Map a [`DaemonResponse`] to the HTTP status code it surfaces on the wire.
///
/// | Category         | HTTP                                              |
/// |------------------|---------------------------------------------------|
/// | `"success"` body | `200`                                             |
/// | classified error | from `error_code.http_status()` (e.g. 400/404/409)|
/// | un-coded error   | `500` (unclassified server-side failure)          |
///
/// Error identity is the machine-readable [`ErrorCode`](super_stt_shared::models::protocol::ErrorCode)
/// the daemon attaches (see `docs/protocol/transport.md`); the earlier
/// substring-on-`message` matcher — which had drifted from the live wire
/// strings — has been retired in favor of it.
pub(crate) fn status_code_for_response(resp: &DaemonResponse) -> StatusCode {
    if resp.status == "success" {
        return StatusCode::OK;
    }
    resp.error_code
        .map_or(StatusCode::INTERNAL_SERVER_ERROR, |code| {
            StatusCode::from_u16(code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
        })
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
    use super_stt_shared::models::protocol::{DaemonResponse, ErrorCode};

    /// The status is derived from the machine-readable `error_code`, not the
    /// human `message`. Covers the classes the retired substring matcher used to
    /// handle: bad-input (400), state conflict (409), and not-found (404).
    #[test]
    fn status_is_derived_from_error_code() {
        let cases = [
            // client named a model/source no installed backend serves →
            // 400 (docs/protocol/endpoints/v1/{active_model,active_backend}.md)
            (ErrorCode::InvalidModel, StatusCode::BAD_REQUEST),
            (ErrorCode::InvalidBackend, StatusCode::BAD_REQUEST),
            (ErrorCode::InvalidAudioTheme, StatusCode::BAD_REQUEST),
            (ErrorCode::UnsupportedLanguage, StatusCode::BAD_REQUEST),
            // request well-formed but daemon state forbids it → 409
            (ErrorCode::RecordingInProgress, StatusCode::CONFLICT),
            (ErrorCode::DownloadInProgress, StatusCode::CONFLICT),
            (ErrorCode::NoSwitchInProgress, StatusCode::CONFLICT),
            (ErrorCode::NotFound, StatusCode::NOT_FOUND),
            (ErrorCode::Internal, StatusCode::INTERNAL_SERVER_ERROR),
        ];
        for (code, expected) in cases {
            let resp = DaemonResponse::error_with_code(code, "human-readable detail");
            assert_eq!(
                status_code_for_response(&resp),
                expected,
                "unexpected status for {code:?}"
            );
        }
    }

    /// The `message` wording never affects the status — only the code does.
    #[test]
    fn message_wording_does_not_affect_status() {
        // A conflict code whose message happens to read like a bad-input error
        // still maps by the code (409), not the words.
        let resp = DaemonResponse::error_with_code(
            ErrorCode::RecordingInProgress,
            "invalid model situation while recording",
        );
        assert_eq!(status_code_for_response(&resp), StatusCode::CONFLICT);
    }

    /// An error with no `error_code` is an unclassified server-side failure.
    #[test]
    fn uncoded_error_is_internal() {
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
