// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperSTTDaemon;
use axum::http::StatusCode;
use serde_json::Value;
use super_stt_shared::models::protocol::{DaemonRequest, DaemonResponse, ErrorCode};

pub(crate) async fn dispatch(daemon: &SuperSTTDaemon, request: DaemonRequest) -> DaemonResponse {
    daemon.handle_command(request).await
}

/// Read the optional top-level `language` field (BCP-47 or `"auto"`) from a
/// request body. Both the mic (`record`) and pre-captured (`transcribe`) paths
/// honor it as a per-request override.
fn language_from_body(data: Option<&Value>) -> Option<String> {
    data.and_then(|d| d.get("language"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub(crate) fn build_request(command: &str, data: Option<Value>) -> DaemonRequest {
    let language = language_from_body(data.as_ref());
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
        language,
        enabled: None,
    }
}

/// Build a `transcribe` (pre-captured audio) request from a `POST /transcribe`
/// body that carries a top-level `audio_data` array. `audio_data` is *moved*
/// out of the body so the (potentially large) sample buffer is never held
/// twice. Returns a coded `400` [`DaemonResponse`] (boxed — the god-struct is
/// large) if `audio_data` fails to deserialize into `[f32]`.
pub(crate) fn build_transcribe_request(
    mut body: Value,
) -> Result<DaemonRequest, Box<DaemonResponse>> {
    let audio_value = body
        .as_object_mut()
        .and_then(|map| map.remove("audio_data"));
    let audio_data: Vec<f32> = match audio_value {
        Some(v) => serde_json::from_value(v).map_err(|e| {
            Box::new(DaemonResponse::error_with_code(
                ErrorCode::InvalidValue,
                &format!("invalid audio_data: {e}"),
            ))
        })?,
        // The caller only routes here when `audio_data` is present, so this is
        // unreachable in practice; treat a vanished field as a bad request.
        None => {
            return Err(Box::new(DaemonResponse::error_with_code(
                ErrorCode::InvalidValue,
                "missing audio_data",
            )));
        }
    };
    let sample_rate = body
        .get("sample_rate")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok());
    let language = language_from_body(Some(&body));
    Ok(DaemonRequest {
        command: "transcribe".to_string(),
        audio_data: Some(audio_data),
        sample_rate,
        client_id: Some(format!("http-cli-{}", uuid::Uuid::new_v4())),
        event_types: None,
        client_info: None,
        since_timestamp: None,
        limit: None,
        event_type: None,
        data: None,
        language,
        enabled: None,
    })
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

    /// A pre-captured `POST /transcribe` body routes to the `transcribe`
    /// command with `audio_data` moved out and `sample_rate`/`language` read
    /// from the top level.
    #[test]
    fn build_transcribe_request_reads_top_level_fields() {
        let body = serde_json::json!({
            "audio_data": [0.1, -0.2, 0.3],
            "sample_rate": 16000,
            "language": "es",
        });
        let req = super::build_transcribe_request(body).expect("valid audio_data");
        assert_eq!(req.command, "transcribe");
        assert_eq!(req.audio_data.as_deref().map(<[f32]>::len), Some(3));
        assert_eq!(req.sample_rate, Some(16000));
        assert_eq!(req.language.as_deref(), Some("es"));
        assert!(
            req.data.is_none(),
            "audio_data is moved out, not duplicated"
        );
    }

    /// Non-numeric `audio_data` is a `400`, not a panic or a silent mic capture.
    #[test]
    fn build_transcribe_request_rejects_non_numeric_audio() {
        let body = serde_json::json!({ "audio_data": ["not", "numbers"] });
        let err = super::build_transcribe_request(body).expect_err("should reject");
        assert_eq!(status_code_for_response(&err), StatusCode::BAD_REQUEST);
    }
}
