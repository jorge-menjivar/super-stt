// SPDX-License-Identifier: GPL-3.0-only
use super::error::{HttpError, HttpResult};
use crate::models::protocol::DaemonResponse;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Bytes;
use hyper::{Method, Request};
use serde::de::DeserializeOwned;

/// All endpoints are served under the `/v1` URL prefix. The request
/// builders below prepend this automatically, so call sites use bare
/// paths like `/ping`, `/transcribe`, `/events` — the actual
/// URL on the wire is `/v1/ping`, `/v1/transcribe`, etc.
pub(crate) const API_PREFIX: &str = "/v1";

/// Body type so a GET/DELETE (empty) and a POST (JSON) share one hyper request
/// type. `http_body_util::Either` supplies the `Body` impl — both arms are
/// `Bytes` bodies with an `Infallible` error — replacing a hand-rolled `unsafe`
/// pin projection (this crate's only `unsafe`).
pub(crate) type RequestBody = http_body_util::Either<Empty<Bytes>, Full<Bytes>>;

pub(crate) fn build_get(path: &str, token: Option<&str>) -> Result<Request<RequestBody>, String> {
    let mut builder = Request::builder()
        .method(Method::GET)
        .uri(format!("http://stt.local{API_PREFIX}{path}"))
        .header("host", "stt.local");
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    builder
        .body(http_body_util::Either::Left(Empty::new()))
        .map_err(|e| format!("Failed to build request: {e}"))
}

pub(crate) fn build_post_json(
    path: &str,
    body: &serde_json::Value,
    token: Option<&str>,
) -> Result<Request<RequestBody>, String> {
    let body_bytes = serde_json::to_vec(body).map_err(|e| format!("Failed to encode body: {e}"))?;
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(format!("http://stt.local{API_PREFIX}{path}"))
        .header("host", "stt.local")
        .header("content-type", "application/json")
        .header("content-length", body_bytes.len().to_string());
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    builder
        .body(http_body_util::Either::Right(Full::new(Bytes::from(
            body_bytes,
        ))))
        .map_err(|e| format!("Failed to build request: {e}"))
}

pub(crate) fn build_delete(
    path: &str,
    token: Option<&str>,
) -> Result<Request<RequestBody>, String> {
    let mut builder = Request::builder()
        .method(Method::DELETE)
        .uri(format!("http://stt.local{API_PREFIX}{path}"))
        .header("host", "stt.local");
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    builder
        .body(http_body_util::Either::Left(Empty::new()))
        .map_err(|e| format!("Failed to build request: {e}"))
}

/// Extract the `data.reason` field from an error body, falling back to
/// `fallback` when the body is malformed or carries no reason.
pub(crate) fn parse_reason(body: &[u8], fallback: &str) -> String {
    let parsed: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    parsed
        .get("data")
        .and_then(|d| d.get("reason"))
        .and_then(|r| r.as_str())
        .unwrap_or(fallback)
        .to_string()
}

/// The `data.reason` of a 401 body. Centralized so the `invalid_session`
/// sites stay consistent.
pub(crate) fn parse_invalid_session_reason(body: &[u8]) -> String {
    parse_reason(body, "unknown")
}

/// **The** conversion from a non-success response to a typed [`HttpError`].
///
/// Every non-2xx path funnels through here so the rule lives in one place.
/// It previously did not: four call sites each re-derived it, and the typed
/// `send_request` path omitted the check entirely — handing error envelopes to
/// the success-type deserializer, which then blamed whichever success field it
/// found missing (a rate-limited uninstall surfaced as ``missing field
/// `uninstalled` at line 1 column 43``).
///
/// `401` keeps its own variant because callers re-authenticate on it; every
/// other status is an operational failure described by [`daemon_error`].
///
/// Note `/auth/request` deliberately does *not* use this: a 4xx there means the
/// user declined consent, which is [`HttpError::AuthDenied`], not a daemon
/// error.
#[must_use]
pub fn error_for_status(status: hyper::StatusCode, body: &[u8]) -> HttpError {
    if status == hyper::StatusCode::UNAUTHORIZED {
        return HttpError::InvalidSession {
            reason: parse_invalid_session_reason(body),
        };
    }
    daemon_error(status, body)
}

/// Build an [`HttpError`] from a non-success, non-401 response body.
///
/// The daemon answers failures with one of several `{"status":"error", …}`
/// envelopes, all of which name the failure in `error_code` (the stable
/// machine identifier — `error` is the registry envelope's legacy spelling of
/// the same value) and/or `message` (human text). None of them share a shape
/// with any endpoint's success type, so a non-2xx body must be turned into an
/// error rather than deserialized.
///
/// Prefer [`error_for_status`] — it adds the `401` case. This is separate only
/// so the daemon's envelope-contract test can assert on the operational
/// mapping directly.
#[must_use]
pub fn daemon_error(status: hyper::StatusCode, body: &[u8]) -> HttpError {
    let parsed: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    let field = |key: &str| {
        parsed
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let code = field("error_code").or_else(|| field("error"));
    let detail = match (code, field("message")) {
        // Envelopes that carry no human text set `message` to the code itself;
        // don't say it twice.
        (Some(code), Some(message)) if code == message => Some(code),
        (Some(code), Some(message)) => Some(format!("{code}: {message}")),
        (Some(text), None) | (None, Some(text)) => Some(text),
        (None, None) => None,
    };
    let status = status.as_u16();
    HttpError::Other(detail.map_or_else(
        || format!("daemon returned HTTP {status}"),
        |detail| format!("{detail} (HTTP {status})"),
    ))
}

/// Map non-success statuses on the `/events` subscribe call to the
/// typed [`HttpError`] surface (so the caller can `match` on
/// `InvalidSession` for re-auth) and pass through 2xx responses
/// unchanged.
pub(crate) async fn check_subscribe_status(
    response: hyper::Response<hyper::body::Incoming>,
) -> HttpResult<hyper::Response<hyper::body::Incoming>> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = collect_body(response).await?;
    Err(error_for_status(status, &body))
}

/// Upper bound on how long a **non-interactive** request waits for the
/// daemon's response headers. A daemon that accepts the connection but
/// never answers — e.g. wedged mid-startup, before its accept loop is
/// serving — would otherwise hang the caller forever. That is exactly how
/// a stuck daemon used to freeze the app's startup probe with no error.
///
/// Deliberately *not* applied to the consent-bearing `/auth/request`
/// (see [`open`]'s `timeout` param): that call is held open while a human
/// clicks Allow/Deny and may type a keyring password, so a machine timer
/// must never race it. Sized generously even for the machine calls so a
/// slow model reload still completes; a daemon that is simply *down*
/// fails fast at `connect()` and never reaches this bound.
pub(crate) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Open an HTTP/1.1 connection over a fresh Unix socket, run one request,
/// and return the streaming response. Shared by every call path.
///
/// `timeout` bounds the wait for the daemon's response headers. Pass
/// `Some(_)` for machine-to-machine calls so a wedged daemon can't hang
/// the caller forever. Pass `None` for `/auth/request`, which the daemon
/// legitimately holds open while the user responds to the consent popup
/// (and possibly a keyring-unlock prompt) — bounding it would cut the user
/// off mid-decision.
pub(crate) async fn open(
    socket_path: &std::path::PathBuf,
    req: hyper::Request<RequestBody>,
    timeout: Option<std::time::Duration>,
) -> HttpResult<hyper::Response<hyper::body::Incoming>> {
    let stream = tokio::net::UnixStream::connect(socket_path)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                HttpError::Other(
                    "Daemon HTTP listener not running. Start the daemon first.".to_string(),
                )
            }
            _ => HttpError::Other(format!("Connection failed: {e}")),
        })?;
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|e| HttpError::Other(format!("HTTP handshake failed: {e}")))?;
    let conn_task = tokio::spawn(async move {
        if let Err(e) = conn.await {
            log::debug!("http connection ended: {e}");
        }
    });
    // On success `conn_task` keeps driving the connection so the body
    // (incl. long-lived `/events` streams) can be read; on timeout we abort
    // it so a dead connection doesn't leak a background task.
    let send = sender.send_request(req);
    let result = match timeout {
        Some(dur) => match tokio::time::timeout(dur, send).await {
            Ok(result) => result,
            Err(_elapsed) => {
                conn_task.abort();
                return Err(HttpError::Other(format!(
                    "Daemon did not respond within {}s (it may be stuck or still starting up)",
                    dur.as_secs()
                )));
            }
        },
        None => send.await,
    };
    result.map_err(|e| HttpError::Other(format!("Request failed: {e}")))
}

/// Collect a response body to bytes.
pub(crate) async fn collect_body(
    response: hyper::Response<hyper::body::Incoming>,
) -> HttpResult<Bytes> {
    Ok(response
        .into_body()
        .collect()
        .await
        .map_err(|e| HttpError::Other(format!("Failed to read response: {e}")))?
        .to_bytes())
}

/// Run a request and deserialize the JSON body into `T`. `401` becomes
/// `HttpError::InvalidSession`.
pub(crate) async fn send_request<T: DeserializeOwned>(
    socket_path: &std::path::PathBuf,
    req: hyper::Request<RequestBody>,
) -> HttpResult<T> {
    send_request_with_timeout(socket_path, req, Some(REQUEST_TIMEOUT)).await
}

/// Like [`send_request`] but with a caller-chosen header timeout. Pass
/// `None` for a long-running call whose response the daemon only sends once
/// the work finishes — e.g. a model switch that streams multi-GB weights
/// first. The fixed [`REQUEST_TIMEOUT`] would otherwise abort the connection
/// mid-switch and cancel the daemon-side load; progress is observed
/// out-of-band via the `download_progress` SSE topic instead.
pub(crate) async fn send_request_with_timeout<T: DeserializeOwned>(
    socket_path: &std::path::PathBuf,
    req: hyper::Request<RequestBody>,
    timeout: Option<std::time::Duration>,
) -> HttpResult<T> {
    let response = open(socket_path, req, timeout).await?;
    let status = response.status();
    let body = collect_body(response).await?;
    // Only a 2xx body is an instance of `T`. Deserializing an error envelope
    // into the success type reports a missing success field instead of the
    // failure the daemon actually named — see [`error_for_status`].
    if !status.is_success() {
        return Err(error_for_status(status, &body));
    }
    serde_json::from_slice::<T>(&body)
        .map_err(|e| HttpError::Other(format!("Failed to parse response: {e}")))
}

/// `GET <path>` → `DaemonResponse`. The standard settings read.
///
/// # Errors
/// Returns [`HttpError::InvalidSession`] on `401`; [`HttpError::Other`] on
/// connection, HTTP, or parse failure.
pub async fn settings_get(
    socket_path: std::path::PathBuf,
    token: &str,
    path: &str,
) -> HttpResult<DaemonResponse> {
    get_json::<DaemonResponse>(socket_path, token, path).await
}

/// `POST <path>` with a JSON body → `DaemonResponse`. The standard settings write.
///
/// # Errors
/// Returns [`HttpError::InvalidSession`] on `401`; [`HttpError::Other`] on
/// connection, HTTP, body encoding, or parse failure.
pub async fn settings_post(
    socket_path: std::path::PathBuf,
    token: &str,
    path: &str,
    body: &serde_json::Value,
) -> HttpResult<DaemonResponse> {
    post_json::<DaemonResponse>(socket_path, token, path, body).await
}

/// Like [`settings_post`] but without the fixed header timeout — for a
/// long-running write whose response the daemon only sends once the work
/// completes (notably `POST /active_model`, a model switch that may stream
/// multi-GB weights first). Bounding it with [`REQUEST_TIMEOUT`] would drop
/// the connection mid-switch and cancel the daemon-side load; callers track
/// progress via the `download_progress` SSE topic instead.
///
/// # Errors
/// Returns [`HttpError::InvalidSession`] on `401`; [`HttpError::Other`] on
/// connection, HTTP, body encoding, or parse failure.
pub async fn settings_post_no_timeout(
    socket_path: std::path::PathBuf,
    token: &str,
    path: &str,
    body: &serde_json::Value,
) -> HttpResult<DaemonResponse> {
    let req = build_post_json(path, body, Some(token))?;
    send_request_with_timeout::<DaemonResponse>(&socket_path, req, None).await
}

/// `DELETE <path>` → `DaemonResponse`.
///
/// # Errors
/// Returns [`HttpError::InvalidSession`] on `401`; [`HttpError::Other`] on
/// connection, HTTP, or parse failure.
pub async fn settings_delete(
    socket_path: std::path::PathBuf,
    token: &str,
    path: &str,
) -> HttpResult<DaemonResponse> {
    delete_json::<DaemonResponse>(socket_path, token, path).await
}

/// `GET <path>` deserialized into `T`. The typed counterpart of
/// [`settings_get`] for endpoints that return a bespoke struct.
///
/// # Errors
/// Returns [`HttpError::InvalidSession`] on `401`; [`HttpError::Other`] on
/// connection, HTTP, or parse failure.
pub async fn get_json<T: DeserializeOwned>(
    socket_path: std::path::PathBuf,
    token: &str,
    path: &str,
) -> HttpResult<T> {
    let req = build_get(path, Some(token))?;
    send_request::<T>(&socket_path, req).await
}

/// `POST <path>` with a JSON body, deserialized into `T`.
///
/// # Errors
/// Returns [`HttpError::InvalidSession`] on `401`; [`HttpError::Other`] on
/// connection, HTTP, body encoding, or parse failure.
pub async fn post_json<T: DeserializeOwned>(
    socket_path: std::path::PathBuf,
    token: &str,
    path: &str,
    body: &serde_json::Value,
) -> HttpResult<T> {
    let req = build_post_json(path, body, Some(token))?;
    send_request::<T>(&socket_path, req).await
}

/// `DELETE <path>` deserialized into `T`.
///
/// # Errors
/// Returns [`HttpError::InvalidSession`] on `401`; [`HttpError::Other`] on
/// connection, HTTP, or parse failure.
pub async fn delete_json<T: DeserializeOwned>(
    socket_path: std::path::PathBuf,
    token: &str,
    path: &str,
) -> HttpResult<T> {
    let req = build_delete(path, Some(token))?;
    send_request::<T>(&socket_path, req).await
}

#[cfg(test)]
mod tests {
    use super::daemon_error;
    use hyper::StatusCode;

    /// A non-2xx body must never reach the `T` deserializer. The daemon's
    /// error envelopes don't share a shape with the endpoint's success type,
    /// so parsing one as `T` reports whichever success field happened to be
    /// missing — burying the actual cause. This is the regression that made a
    /// rate-limited uninstall surface as
    /// ``missing field `uninstalled` at line 1 column 43``.
    #[test]
    fn non_success_status_reports_the_daemon_identifier_not_a_parse_error() {
        let e = daemon_error(
            StatusCode::TOO_MANY_REQUESTS,
            br#"{"status":"error","message":"rate_limited"}"#,
        );
        assert_eq!(e.to_string(), "rate_limited (HTTP 429)");
        assert!(
            !e.to_string().contains("missing field"),
            "the endpoint's success schema must not be blamed for a daemon error"
        );
    }

    /// The registry envelope carries the identifier in `error_code`, plus the
    /// legacy `error` spelling of the same value. One mention is enough.
    #[test]
    fn registry_envelope_uses_error_code_without_repeating_the_legacy_key() {
        let e = daemon_error(
            StatusCode::NOT_FOUND,
            br#"{"status":"error","error_code":"not_found","error":"not_found"}"#,
        );
        assert_eq!(e.to_string(), "not_found (HTTP 404)");
    }

    /// When the daemon supplies both a stable code and a human message, keep
    /// both: the code is what a maintainer greps for, the message is what tells
    /// the user *why* (e.g. the underlying io error behind `remove_failed`).
    #[test]
    fn code_and_message_are_both_preserved() {
        let e = daemon_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            br#"{"status":"error","error_code":"remove_failed","error":"remove_failed","message":"Permission denied (os error 13)"}"#,
        );
        assert_eq!(
            e.to_string(),
            "remove_failed: Permission denied (os error 13) (HTTP 500)"
        );
    }

    /// `dispatch_command` answers with a `DaemonResponse` whose `message` is
    /// the human text callers used to surface via `require_success`. That text
    /// must survive the earlier status check.
    #[test]
    fn daemon_response_error_keeps_its_human_message() {
        let e = daemon_error(
            StatusCode::BAD_REQUEST,
            br#"{"status":"error","message":"Model not loaded","error_code":"model_not_loaded"}"#,
        );
        assert_eq!(
            e.to_string(),
            "model_not_loaded: Model not loaded (HTTP 400)"
        );
    }

    /// A body that isn't the expected envelope at all (empty, HTML from a
    /// stray proxy, truncated JSON) still has to name the status rather than
    /// invent a field-level complaint.
    #[test]
    fn unrecognized_body_falls_back_to_the_bare_status() {
        assert_eq!(
            daemon_error(StatusCode::BAD_GATEWAY, b"").to_string(),
            "daemon returned HTTP 502"
        );
        assert_eq!(
            daemon_error(StatusCode::FORBIDDEN, b"<html>nope</html>").to_string(),
            "daemon returned HTTP 403"
        );
        // Present-but-empty strings are as useless as absent ones.
        assert_eq!(
            daemon_error(StatusCode::CONFLICT, br#"{"status":"error","message":""}"#).to_string(),
            "daemon returned HTTP 409"
        );
    }
}
