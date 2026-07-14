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

/// Extract the `data.reason` field from a 401 response body. Falls
/// back to `"unknown"` if the body is malformed. Centralized so the
/// three `invalid_session` sites stay consistent.
pub(crate) fn parse_invalid_session_reason(body: &[u8]) -> String {
    let parsed: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    parsed
        .get("data")
        .and_then(|d| d.get("reason"))
        .and_then(|r| r.as_str())
        .unwrap_or("unknown")
        .to_string()
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
    if status == hyper::StatusCode::UNAUTHORIZED {
        return Err(HttpError::InvalidSession {
            reason: parse_invalid_session_reason(&body),
        });
    }
    let parsed: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    Err(HttpError::Other(format!(
        "events subscribe failed (status {status}): {parsed}"
    )))
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
    if status == hyper::StatusCode::UNAUTHORIZED {
        return Err(HttpError::InvalidSession {
            reason: parse_invalid_session_reason(&body),
        });
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
