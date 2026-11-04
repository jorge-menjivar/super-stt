// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperTTSDaemon;
use axum::http::StatusCode;
use serde_json::Value;
use super_tts_shared::models::protocol::{DaemonRequest, DaemonResponse};

pub(crate) async fn dispatch(daemon: &SuperTTSDaemon, request: DaemonRequest) -> DaemonResponse {
    daemon.handle_command(request).await
}

/// Read the optional top-level `language` field (BCP-47 or `"auto"`) from a
/// request body, which every command honors as a per-request override.
fn language_from_body(data: Option<&Value>) -> Option<String> {
    data.and_then(|d| d.get("language"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub(crate) fn build_request(command: &str, data: Option<Value>) -> DaemonRequest {
    let language = language_from_body(data.as_ref());
    DaemonRequest {
        command: command.to_string(),
        client_id: Some(format!("http-cli-{}", uuid::Uuid::new_v4())),
        event_types: None,
        client_info: None,
        since_timestamp: None,
        limit: None,
        event_type: None,
        data,
        language,
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
/// Error identity is the machine-readable [`ErrorCode`](super_tts_shared::models::protocol::ErrorCode)
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
    daemon: &SuperTTSDaemon,
    command: &str,
    data: Option<Value>,
) -> (StatusCode, [(&'static str, &'static str); 1], String) {
    let resp = dispatch(daemon, build_request(command, data)).await;
    json_response(&resp)
}

/// Answer a dispatched command with a *narrow* success body.
///
/// The command bus speaks one [`DaemonResponse`] carrying every field any
/// command might set. An endpoint answers with the handful it actually fills,
/// and `success` names exactly those — so the type an endpoint publishes in the
/// `OpenAPI` document is the type its handler builds. A schema cannot claim a
/// field the handler never sets, or omit one it does, because there is no
/// second description to disagree with.
///
/// Failures keep the envelope the daemon built, status code and all: error
/// identity is `error_code`, and mapping it is [`json_response`]'s job.
pub(crate) fn narrowed<T: serde::Serialize>(
    resp: DaemonResponse,
    success: impl FnOnce(DaemonResponse) -> T,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    if resp.status != "success" {
        return json_response(&resp).into_response();
    }
    let body = serde_json::to_string(&success(resp))
        .unwrap_or_else(|_| String::from(r#"{"status":"error"}"#));
    (StatusCode::OK, [("content-type", "application/json")], body).into_response()
}

/// [`narrowed`] for the commonest shape of all: an acknowledgement carrying the
/// daemon's sentence and nothing else.
pub(crate) async fn ack(
    daemon: &SuperTTSDaemon,
    command: &str,
    data: Option<Value>,
) -> axum::response::Response {
    let resp = dispatch(daemon, build_request(command, data)).await;
    narrowed(resp, |r| crate::daemon::http::wire::Ack {
        status: "success",
        message: r.message,
    })
}

#[cfg(test)]
mod tests {
    use super::status_code_for_response;
    use axum::http::StatusCode;
    use super_tts_shared::models::protocol::{DaemonResponse, ErrorCode};

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
            (ErrorCode::SpeechInProgress, StatusCode::CONFLICT),
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
            ErrorCode::SpeechInProgress,
            "invalid model situation while speaking",
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
