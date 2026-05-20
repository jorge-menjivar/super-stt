// SPDX-License-Identifier: GPL-3.0-only
//! HTTP client for the new daemon protocol.
//!
//! Lives **side-by-side** with `client.rs` (the legacy length-prefix Unix
//! socket client). Nothing in `client.rs` is touched; this module is
//! additive and can be removed when/if the legacy path is retired.
//!
//! The transport is HTTP/1.1 over a Unix domain socket
//! (`super_stt_shared::validation::get_http_socket_path()`). Each request
//! opens a fresh `tokio::net::UnixStream`, runs `hyper::client::conn::http1`
//! over it, and parses the JSON response into a `DaemonResponse`.
//!
//! Authentication is **stubbed** in v1: requests are sent with no
//! `Authorization` header and the daemon accepts everything. The
//! libcosmic consent flow + bearer-token enforcement is a follow-up.

use crate::models::protocol::DaemonResponse;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Bytes;
use hyper::{Method, Request};
use serde::{Deserialize, de::DeserializeOwned};
use std::path::PathBuf;
use tokio::net::UnixStream;

/// Result type for all HTTP-protocol calls.
pub type HttpResult<T> = Result<T, String>;

/// Open an HTTP/1.1 connection over a fresh Unix socket and run one
/// request. Generic over the deserialized response type — pass
/// `DaemonResponse` for the legacy-shaped endpoints, or a custom struct
/// (e.g. `ActiveModelStatus`) for endpoints with their own response
/// shape.
///
/// On `401 Unauthorized`, the response body is parsed for the
/// `data.reason` field and the function returns
/// `Err("invalid_session (<reason>)")` so callers can re-auth. Other
/// HTTP statuses (2xx, 4xx other than 401, 5xx) are deserialized into
/// `T` — for `DaemonResponse` that includes the `status: "error"`
/// variant clients can inspect.
async fn send_request<T: DeserializeOwned>(
    socket_path: &PathBuf,
    req: Request<RequestBody>,
) -> HttpResult<T> {
    let stream = UnixStream::connect(socket_path)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                "Daemon HTTP listener not running. Start the daemon first.".to_string()
            }
            _ => format!("Connection failed: {e}"),
        })?;

    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|e| format!("HTTP handshake failed: {e}"))?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            log::debug!("http connection ended: {e}");
        }
    });

    let response = sender
        .send_request(req)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?
        .to_bytes();

    if status == hyper::StatusCode::UNAUTHORIZED {
        let parsed: serde_json::Value =
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        let reason = parsed
            .get("data")
            .and_then(|d| d.get("reason"))
            .and_then(|r| r.as_str())
            .unwrap_or("unknown");
        return Err(format!("invalid_session ({reason})"));
    }

    serde_json::from_slice::<T>(&body).map_err(|e| format!("Failed to parse response: {e}"))
}

/// Body type wrapper so we can use either an empty body (GET) or a JSON body
/// (POST) through the same hyper builder.
enum RequestBody {
    Empty(Empty<Bytes>),
    Full(Full<Bytes>),
}

impl hyper::body::Body for RequestBody {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, hyper::Error>>> {
        // SAFETY: pin projection — RequestBody is a plain enum of two
        // body types and we only forward to their poll_frame.
        let this = unsafe { self.get_unchecked_mut() };
        match this {
            RequestBody::Empty(b) => unsafe { std::pin::Pin::new_unchecked(b) }
                .poll_frame(cx)
                .map(|opt| opt.map(|r| r.map_err(|never| match never {}))),
            RequestBody::Full(b) => unsafe { std::pin::Pin::new_unchecked(b) }
                .poll_frame(cx)
                .map(|opt| opt.map(|r| r.map_err(|never| match never {}))),
        }
    }
}

fn build_get(path: &str, token: Option<&str>) -> Result<Request<RequestBody>, String> {
    let mut builder = Request::builder()
        .method(Method::GET)
        .uri(format!("http://stt.local{path}"))
        .header("host", "stt.local");
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    builder
        .body(RequestBody::Empty(Empty::new()))
        .map_err(|e| format!("Failed to build request: {e}"))
}

fn build_post_json(
    path: &str,
    body: &serde_json::Value,
    token: Option<&str>,
) -> Result<Request<RequestBody>, String> {
    let body_bytes = serde_json::to_vec(body).map_err(|e| format!("Failed to encode body: {e}"))?;
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(format!("http://stt.local{path}"))
        .header("host", "stt.local")
        .header("content-type", "application/json")
        .header("content-length", body_bytes.len().to_string());
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    builder
        .body(RequestBody::Full(Full::new(Bytes::from(body_bytes))))
        .map_err(|e| format!("Failed to build request: {e}"))
}

/// Successful `POST /auth/request` payload.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthOk {
    pub session_token: String,
    pub scope: String,
    pub expires_at: String,
}

/// `POST /auth/request` — always runs the consent popup and mints a
/// fresh session token. Used by clients that have no cached token (or
/// whose cached token was invalidated by `401 invalid_session`).
/// Clients with a valid cached token never call this; they go
/// straight to `/ping`/`/events`/etc. with the bearer header.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable, the
/// user denies the request, or the popup is dismissed.
pub async fn auth_request(socket_path: PathBuf, app_name: &str, scope: &str) -> HttpResult<AuthOk> {
    let body = serde_json::json!({
        "app_name": app_name,
        "scope":    scope,
        "version":  env!("CARGO_PKG_VERSION"),
    });
    let req = build_post_json("/auth/request", &body, None)?;

    // /auth/request returns its own JSON shape (not a `DaemonResponse`),
    // so we issue the request directly here and parse on top of the raw
    // body.
    let stream = UnixStream::connect(&socket_path)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                "Daemon HTTP listener not running. Start the daemon first.".to_string()
            }
            _ => format!("Connection failed: {e}"),
        })?;
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|e| format!("HTTP handshake failed: {e}"))?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            log::debug!("http connection ended: {e}");
        }
    });

    let response = sender
        .send_request(req)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?
        .to_bytes();

    if !status.is_success() {
        let parsed: serde_json::Value =
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        let reason = parsed
            .get("data")
            .and_then(|d| d.get("reason"))
            .and_then(|r| r.as_str())
            .unwrap_or("auth_denied");
        return Err(format!("auth_denied ({reason})"));
    }

    serde_json::from_slice::<AuthOk>(&body).map_err(|e| format!("Failed to parse auth_ok: {e}"))
}

/// `GET /ping` — liveness check.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable or the
/// response can't be parsed.
pub async fn ping(socket_path: PathBuf, token: &str) -> HttpResult<String> {
    let req = build_get("/ping", Some(token))?;
    let resp = send_request::<DaemonResponse>(&socket_path, req).await?;
    Ok(resp
        .message
        .unwrap_or_else(|| "Daemon is running".to_string()))
}

/// `GET /status` — current model + device.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable or the
/// response can't be parsed.
pub async fn status(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    let req = build_get("/status", Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

/// Options for [`transcribe`]. v1 only wires the daemon-mic capture path
/// (no `audio_data`); pre-captured audio is a follow-up.
#[derive(Debug, Default)]
pub struct TranscribeOptions {
    pub write_mode: bool,
    pub stop_mode: Option<String>,
    pub wait: bool,
}

/// `POST /transcribe` — start a daemon-mic recording. Non-streaming
/// variant: collapses the entire NDJSON event stream into a single
/// final result.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable or the
/// response can't be parsed.
pub async fn transcribe(
    socket_path: PathBuf,
    token: &str,
    opts: TranscribeOptions,
) -> HttpResult<DaemonResponse> {
    use futures_util::StreamExt;
    let mut stream = Box::pin(transcribe_stream(socket_path, token, opts).await?);
    let mut last_preview = String::new();
    while let Some(event) = stream.next().await {
        match event {
            TranscribeEvent::Preview(t) => last_preview = t,
            TranscribeEvent::Done(text) => {
                return Ok(DaemonResponse::success().with_transcription(text));
            }
            TranscribeEvent::Error(msg) => {
                return Ok(DaemonResponse::error(&msg));
            }
        }
    }
    // Stream ended without a terminal event; surface what we have.
    if last_preview.is_empty() {
        Ok(DaemonResponse::error(
            "transcribe stream ended unexpectedly",
        ))
    } else {
        Ok(DaemonResponse::success().with_transcription(last_preview))
    }
}

/// One event from the `/transcribe` NDJSON stream.
///
/// Matches the daemon's wire-level event types — `preview` for
/// in-flight text, `done` for the final transcription, `error` for a
/// daemon-side failure. Mirrors the Mistral realtime event-stream
/// pattern (one typed JSON object per line).
#[derive(Debug, Clone)]
pub enum TranscribeEvent {
    /// Incremental preview text (full text so far, not a delta).
    Preview(String),
    /// Final transcription, after the recording stopped and the model
    /// produced its result. The string is the transcribed text or an
    /// empty string if no speech was detected.
    Done(String),
    /// Daemon-side error. Recording is no longer running.
    Error(String),
}

/// `POST /transcribe` — start a daemon-mic recording, returning a stream
/// of [`TranscribeEvent`]s as the recording progresses.
///
/// The connection is held open by the daemon: each preview update
/// arrives as a `Preview(text)` event, then a single terminal `Done`
/// or `Error` event is emitted before the stream ends. Dropping the
/// stream early closes the underlying connection, which the daemon
/// treats as a manual stop signal.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable or the
/// initial request can't be sent. Errors *during* the stream (e.g.
/// daemon-side failure) are emitted as a `TranscribeEvent::Error` item.
pub async fn transcribe_stream(
    socket_path: PathBuf,
    token: &str,
    opts: TranscribeOptions,
) -> HttpResult<impl futures_util::Stream<Item = TranscribeEvent> + Send + 'static> {
    use http_body_util::BodyStream;
    use hyper::body::Frame;

    let mut data = serde_json::json!({
        "write_mode": opts.write_mode,
        "wait":       opts.wait,
    });
    if let Some(mode) = opts.stop_mode {
        data["stop_mode"] = serde_json::Value::String(mode);
    }
    let req = build_post_json("/transcribe", &data, Some(token))?;

    let stream = UnixStream::connect(&socket_path)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                "Daemon HTTP listener not running. Start the daemon first.".to_string()
            }
            _ => format!("Connection failed: {e}"),
        })?;
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|e| format!("HTTP handshake failed: {e}"))?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            log::debug!("http connection ended: {e}");
        }
    });

    let response = sender
        .send_request(req)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    let status = response.status();
    if status == hyper::StatusCode::UNAUTHORIZED {
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?
            .to_bytes();
        let parsed: serde_json::Value =
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        let reason = parsed
            .get("data")
            .and_then(|d| d.get("reason"))
            .and_then(|r| r.as_str())
            .unwrap_or("unknown");
        return Err(format!("invalid_session ({reason})"));
    }

    // Parse Server-Sent Events as they arrive. Each event is a block
    // of `field: value\n` lines terminated by a blank line (`\n\n`).
    // We collect bytes into a buffer and split on the blank-line
    // boundary; for each block we extract the `event:` and `data:`
    // fields and emit a typed `TranscribeEvent`.
    let body_stream = BodyStream::new(response.into_body());
    let event_stream = async_stream::stream! {
        let mut buffer: Vec<u8> = Vec::new();
        let mut body_stream = body_stream;
        use futures_util::StreamExt;
        while let Some(frame_res) = body_stream.next().await {
            let frame: Frame<_> = match frame_res {
                Ok(f) => f,
                Err(e) => {
                    yield TranscribeEvent::Error(format!("body read error: {e}"));
                    return;
                }
            };
            if let Ok(data) = frame.into_data() {
                buffer.extend_from_slice(&data);
                while let Some(boundary) = find_blank_line(&buffer) {
                    let block_bytes: Vec<u8> = buffer.drain(..boundary.end).collect();
                    let block_text = match std::str::from_utf8(&block_bytes[..boundary.start]) {
                        Ok(s) => s,
                        Err(e) => {
                            yield TranscribeEvent::Error(format!(
                                "non-utf8 SSE block: {e}"
                            ));
                            continue;
                        }
                    };
                    if let Some(ev) = parse_sse_block(block_text) {
                        yield ev;
                    }
                }
            }
        }
    };

    Ok(event_stream)
}

/// Returned span describes the bytes belonging to ONE SSE block.
/// `start` is the byte index of the blank line, `end` is one past the
/// final `\n` that closes the block.
struct BlankLineBoundary {
    start: usize,
    end: usize,
}

/// Find the boundary between the current SSE block and the next.
/// Per the SSE spec, blocks are separated by a blank line — `\n\n`
/// (LF) or `\r\n\r\n` (CRLF). We accept both.
fn find_blank_line(buffer: &[u8]) -> Option<BlankLineBoundary> {
    let mut i = 0;
    while i + 1 < buffer.len() {
        // \n\n
        if buffer[i] == b'\n' && buffer[i + 1] == b'\n' {
            return Some(BlankLineBoundary {
                start: i,
                end: i + 2,
            });
        }
        // \r\n\r\n
        if i + 3 < buffer.len()
            && buffer[i] == b'\r'
            && buffer[i + 1] == b'\n'
            && buffer[i + 2] == b'\r'
            && buffer[i + 3] == b'\n'
        {
            return Some(BlankLineBoundary {
                start: i,
                end: i + 4,
            });
        }
        i += 1;
    }
    None
}

/// Parse one SSE block (text before the blank-line boundary) into a
/// `TranscribeEvent`. Returns `None` for blocks we don't recognize
/// (heartbeats, comments, unknown event types).
fn parse_sse_block(block: &str) -> Option<TranscribeEvent> {
    let mut event_name: Option<&str> = None;
    let mut data = String::new();
    for raw_line in block.split('\n') {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if line.starts_with(':') {
            continue; // SSE comment / heartbeat
        }
        if let Some(rest) = line.strip_prefix("event:") {
            event_name = Some(rest.trim_start());
        } else if let Some(rest) = line.strip_prefix("data:") {
            // Per spec, multiple data: lines concatenate with \n.
            // Daemon emits one data: line per event so this rarely
            // matters, but we handle it correctly.
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        }
        // Other fields (id, retry) are ignored — we don't need them.
    }
    let payload: serde_json::Value = serde_json::from_str(&data).unwrap_or(serde_json::Value::Null);
    match event_name {
        Some("preview") => {
            let text = payload
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(TranscribeEvent::Preview(text))
        }
        Some("done") => {
            let text = payload
                .get("transcription")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(TranscribeEvent::Done(text))
        }
        Some("error") => {
            let msg = payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("transcribe error")
                .to_string();
            Some(TranscribeEvent::Error(msg))
        }
        _ => None,
    }
}

/// `POST /transcribe/stop` — stop an in-flight daemon-mic recording.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable or the
/// response can't be parsed.
pub async fn transcribe_stop(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    let req = build_post_json("/transcribe/stop", &serde_json::json!({}), Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

// =============================================================================
// Settings-scope endpoints
// =============================================================================
//
// Free functions matching the legacy `client.rs` shape. Each takes
// `(socket_path, token, ...args)` and returns a typed result —
// `DaemonResponse` for endpoints whose response is already
// DaemonResponse-shaped, or a custom struct (e.g. `ActiveModelStatus`)
// for composed endpoints.

// ---- /active_model (composed shape) ----

/// Wire shape returned by `GET /active_model`.
#[derive(Debug, Clone, Deserialize)]
pub struct ActiveModelStatus {
    pub status: String,
    pub active_model: ActiveModelPayload,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActiveModelPayload {
    pub current: ActiveModelCurrent,
    pub switch: Option<ActiveModelSwitch>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActiveModelCurrent {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub source: Option<String>,
    #[serde(default)]
    pub loaded: bool,
    pub device: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActiveModelSwitch {
    pub phase: String,
    pub target: serde_json::Value,
    pub started_at: Option<String>,
    pub download: Option<ActiveModelDownload>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActiveModelDownload {
    pub current_file: String,
    pub file_index: usize,
    pub total_files: usize,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub percentage: f32,
    pub eta_seconds: Option<u64>,
}

/// `GET /active_model` — returns the composed `{ active_model: { current,
/// switch } }` payload, deserialized into `ActiveModelStatus`.
///
/// # Errors
/// Returns an error if the daemon isn't reachable or the response can't be parsed.
pub async fn get_active_model(socket_path: PathBuf, token: &str) -> HttpResult<ActiveModelStatus> {
    let req = build_get("/active_model", Some(token))?;
    send_request::<ActiveModelStatus>(&socket_path, req).await
}

/// `POST /active_model` — start switching the active STT model.
///
/// # Errors
/// Returns an error if the daemon isn't reachable or the response can't be parsed.
pub async fn set_active_model(
    socket_path: PathBuf,
    token: &str,
    model: &str,
    provider: &str,
    source: Option<&str>,
) -> HttpResult<DaemonResponse> {
    let mut body = serde_json::json!({ "model": model, "provider": provider });
    if let Some(s) = source {
        body["source"] = serde_json::Value::String(s.to_string());
    }
    let req = build_post_json("/active_model", &body, Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

/// `POST /active_model/cancel` — abort an in-flight model switch.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn cancel_set_active_model(
    socket_path: PathBuf,
    token: &str,
) -> HttpResult<DaemonResponse> {
    let req = build_post_json("/active_model/cancel", &serde_json::json!({}), Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

// ---- /models ----

/// `GET /models` — list available models.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn list_models(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    let req = build_get("/models", Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

// ---- /active_device ----

/// `GET /active_device` — read current device + GPU memory.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn get_active_device(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    let req = build_get("/active_device", Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

/// `POST /active_device` — switch CPU vs CUDA.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn set_active_device(
    socket_path: PathBuf,
    token: &str,
    device: &str,
) -> HttpResult<DaemonResponse> {
    let req = build_post_json(
        "/active_device",
        &serde_json::json!({ "device": device }),
        Some(token),
    )?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

// ---- /audio_theme ----

/// `GET /audio_theme` — read current audio cue theme.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn get_audio_theme(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    let req = build_get("/audio_theme", Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

/// `POST /audio_theme` — set the audio cue theme.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn set_audio_theme(
    socket_path: PathBuf,
    token: &str,
    theme: &str,
) -> HttpResult<DaemonResponse> {
    let req = build_post_json(
        "/audio_theme",
        &serde_json::json!({ "theme": theme }),
        Some(token),
    )?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

/// `POST /audio_theme/test` — play start/stop cues for the current theme.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn test_audio_theme(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    let req = build_post_json("/audio_theme/test", &serde_json::json!({}), Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

/// `GET /audio_themes` — list available themes.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn list_audio_themes(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    let req = build_get("/audio_themes", Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

// ---- /volume ----

/// `GET /volume` — read current cue volume (0–100).
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn get_volume(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    let req = build_get("/volume", Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

/// `POST /volume` — set cue volume.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn set_volume(
    socket_path: PathBuf,
    token: &str,
    volume: u8,
) -> HttpResult<DaemonResponse> {
    let req = build_post_json(
        "/volume",
        &serde_json::json!({ "volume": volume }),
        Some(token),
    )?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

// ---- /recording_stop_mode ----

/// `GET /recording_stop_mode` — read default stop mode.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn get_recording_stop_mode(
    socket_path: PathBuf,
    token: &str,
) -> HttpResult<DaemonResponse> {
    let req = build_get("/recording_stop_mode", Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

/// `POST /recording_stop_mode` — set default stop mode.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn set_recording_stop_mode(
    socket_path: PathBuf,
    token: &str,
    mode: &str,
) -> HttpResult<DaemonResponse> {
    let req = build_post_json(
        "/recording_stop_mode",
        &serde_json::json!({ "mode": mode }),
        Some(token),
    )?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

// ---- /write_method ----

/// `GET /write_method` — read current keyboard write method.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn get_write_method(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    let req = build_get("/write_method", Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

/// `POST /write_method` — set keyboard write method.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn set_write_method(
    socket_path: PathBuf,
    token: &str,
    method: &str,
) -> HttpResult<DaemonResponse> {
    let req = build_post_json(
        "/write_method",
        &serde_json::json!({ "method": method }),
        Some(token),
    )?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

// ---- /preview_typing ----

/// `GET /preview_typing` — read preview-typing flag.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn get_preview_typing(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    let req = build_get("/preview_typing", Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

/// `POST /preview_typing` — set preview-typing flag.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn set_preview_typing(
    socket_path: PathBuf,
    token: &str,
    enabled: bool,
) -> HttpResult<DaemonResponse> {
    let req = build_post_json(
        "/preview_typing",
        &serde_json::json!({ "enabled": enabled }),
        Some(token),
    )?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

// ---- /allow_online_models ----

/// `GET /allow_online_models` — read online-models gate.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn get_allow_online_models(
    socket_path: PathBuf,
    token: &str,
) -> HttpResult<DaemonResponse> {
    let req = build_get("/allow_online_models", Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

/// `POST /allow_online_models` — set online-models gate.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn set_allow_online_models(
    socket_path: PathBuf,
    token: &str,
    enabled: bool,
) -> HttpResult<DaemonResponse> {
    let req = build_post_json(
        "/allow_online_models",
        &serde_json::json!({ "enabled": enabled }),
        Some(token),
    )?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

// ---- /custom_models_dir ----

/// `GET /custom_models_dir` — read configured custom-models directory.
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn get_custom_models_dir(
    socket_path: PathBuf,
    token: &str,
) -> HttpResult<DaemonResponse> {
    let req = build_get("/custom_models_dir", Some(token))?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

/// `POST /custom_models_dir` — set custom-models directory (None to clear).
///
/// # Errors
/// Returns an error if the daemon isn't reachable.
pub async fn set_custom_models_dir(
    socket_path: PathBuf,
    token: &str,
    path: Option<&str>,
) -> HttpResult<DaemonResponse> {
    let req = build_post_json(
        "/custom_models_dir",
        &serde_json::json!({ "path": path }),
        Some(token),
    )?;
    send_request::<DaemonResponse>(&socket_path, req).await
}

// ===========================================================================
// /events — widget SSE subscription
// ===========================================================================

/// One event as it arrives over the daemon's `GET /events` SSE stream.
/// `name` is the SSE `event:` line value (matches `Topic::as_str()` on the
/// daemon side); `payload` is the parsed JSON body. Callers route on
/// `name` and project the payload into the topic-specific shape they
/// expect (see `docs/protocol/widget.md` §"Topics" for the schema).
#[derive(Debug, Clone)]
pub struct WidgetEvent {
    pub name: String,
    pub payload: serde_json::Value,
}

/// `GET /events?topics=...` — open the widget SSE stream and yield each
/// daemon event as a [`WidgetEvent`]. Connection stays open until the
/// daemon disconnects (e.g. on `revoked`) or the returned stream is
/// dropped (which closes the underlying connection).
///
/// `topics` is the comma-joined list emitted as the query value. Empty
/// `topics` is rejected by the daemon with `400 invalid_topic`.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable, the
/// initial request fails, or the daemon returns 401 `invalid_session`.
/// Errors *during* the stream (e.g. body read failure) are surfaced as
/// `WidgetEvent { name: "error", ... }` items so the caller doesn't
/// have to plumb a `Result` through the stream type.
pub async fn events_stream(
    socket_path: PathBuf,
    token: &str,
    topics: &[&str],
) -> HttpResult<impl futures_util::Stream<Item = WidgetEvent> + Send + 'static> {
    if topics.is_empty() {
        return Err("events_stream requires at least one topic".to_string());
    }
    let req = build_events_request(token, topics)?;
    let response = open_http_unix(&socket_path, req).await?;
    check_subscribe_status(response)
        .await
        .map(parse_widget_event_stream)
}

fn build_events_request(token: &str, topics: &[&str]) -> Result<Request<RequestBody>, String> {
    let topics_csv = topics.join(",");
    Request::builder()
        .method(Method::GET)
        .uri(format!("http://stt.local/events?topics={topics_csv}"))
        .header("host", "stt.local")
        .header("accept", "text/event-stream")
        .header("authorization", format!("Bearer {token}"))
        .body(RequestBody::Empty(Empty::new()))
        .map_err(|e| format!("Failed to build request: {e}"))
}

/// Open a fresh HTTP/1 connection over the daemon's Unix socket and
/// run a single request. Returns the response (still streaming) so
/// long-lived endpoints like `/events` can keep reading frames.
async fn open_http_unix(
    socket_path: &PathBuf,
    req: Request<RequestBody>,
) -> HttpResult<hyper::Response<hyper::body::Incoming>> {
    let stream = UnixStream::connect(socket_path)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                "Daemon HTTP listener not running. Start the daemon first.".to_string()
            }
            _ => format!("Connection failed: {e}"),
        })?;
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|e| format!("HTTP handshake failed: {e}"))?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            log::debug!("http connection ended: {e}");
        }
    });
    sender
        .send_request(req)
        .await
        .map_err(|e| format!("Request failed: {e}"))
}

/// Map non-success statuses on the `/events` subscribe call to typed
/// error strings (so the caller can detect `invalid_session` for
/// re-auth) and pass through 2xx responses unchanged.
async fn check_subscribe_status(
    response: hyper::Response<hyper::body::Incoming>,
) -> HttpResult<hyper::Response<hyper::body::Incoming>> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?
        .to_bytes();
    let parsed: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    if status == hyper::StatusCode::UNAUTHORIZED {
        let reason = parsed
            .get("data")
            .and_then(|d| d.get("reason"))
            .and_then(|r| r.as_str())
            .unwrap_or("unknown");
        return Err(format!("invalid_session ({reason})"));
    }
    Err(format!(
        "events subscribe failed (status {status}): {parsed}"
    ))
}

/// Wrap an SSE response body in an async stream that yields one
/// [`WidgetEvent`] per `event:` block. Reuses [`find_blank_line`] for
/// SSE framing and [`parse_widget_sse_block`] for the per-block
/// `event:` / `data:` extraction.
fn parse_widget_event_stream(
    response: hyper::Response<hyper::body::Incoming>,
) -> impl futures_util::Stream<Item = WidgetEvent> + Send + 'static {
    use http_body_util::BodyStream;
    use hyper::body::Frame;
    let body_stream = BodyStream::new(response.into_body());
    async_stream::stream! {
        let mut buffer: Vec<u8> = Vec::new();
        let mut body_stream = body_stream;
        use futures_util::StreamExt;
        while let Some(frame_res) = body_stream.next().await {
            let frame: Frame<_> = match frame_res {
                Ok(f) => f,
                Err(e) => {
                    yield WidgetEvent {
                        name: "error".to_string(),
                        payload: serde_json::json!({
                            "message": format!("body read error: {e}"),
                        }),
                    };
                    return;
                }
            };
            if let Ok(data) = frame.into_data() {
                buffer.extend_from_slice(&data);
                while let Some(boundary) = find_blank_line(&buffer) {
                    let block_bytes: Vec<u8> = buffer.drain(..boundary.end).collect();
                    let block_text = match std::str::from_utf8(&block_bytes[..boundary.start]) {
                        Ok(s) => s,
                        Err(e) => {
                            yield WidgetEvent {
                                name: "error".to_string(),
                                payload: serde_json::json!({
                                    "message": format!("non-utf8 SSE block: {e}"),
                                }),
                            };
                            continue;
                        }
                    };
                    if let Some(ev) = parse_widget_sse_block(block_text) {
                        yield ev;
                    }
                }
            }
        }
    }
}

/// Parse one SSE block into a [`WidgetEvent`].
///
/// A normal `event: <name>\ndata: <json>` block produces an event with
/// the named payload. A *comment-only* block (lines starting with `:`,
/// per the SSE spec) is surfaced as a synthetic
/// `WidgetEvent { name: "keepalive", payload: Null }` so the
/// subscription helper's idle deadline (in
/// `super-stt-shared::daemon::widget_subscription`) is reset on every
/// keepalive — without this synthetic event, the daemon's `:
/// keepalive\n\n` heartbeats would be silently swallowed and the
/// helper would tear the stream down every minute.
///
/// A truly empty block (no event, no data, no comment) returns `None`
/// and is dropped.
fn parse_widget_sse_block(block: &str) -> Option<WidgetEvent> {
    let mut event_name: Option<&str> = None;
    let mut data = String::new();
    let mut saw_comment = false;
    for raw_line in block.split('\n') {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if line.starts_with(':') {
            saw_comment = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            event_name = Some(rest.trim_start());
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        }
    }
    if let Some(name) = event_name {
        let payload: serde_json::Value =
            serde_json::from_str(&data).unwrap_or(serde_json::Value::Null);
        return Some(WidgetEvent {
            name: name.to_string(),
            payload,
        });
    }
    // Comment-only block (no event/data). Surface a keepalive so the
    // helper sees wire activity and refreshes its idle deadline.
    if saw_comment {
        return Some(WidgetEvent {
            name: "keepalive".to_string(),
            payload: serde_json::Value::Null,
        });
    }
    None
}

#[cfg(test)]
mod widget_sse_parser_tests {
    use super::*;

    #[test]
    fn parses_keepalive_comment_as_synthetic_event() {
        let evt = parse_widget_sse_block(": keepalive").expect("comment yields keepalive");
        assert_eq!(evt.name, "keepalive");
        assert!(evt.payload.is_null());
    }

    #[test]
    fn parses_named_event_with_json_payload() {
        let block = "event: subscribed\ndata: {\"client_id\":\"abc\"}";
        let evt = parse_widget_sse_block(block).expect("event yields");
        assert_eq!(evt.name, "subscribed");
        assert_eq!(evt.payload["client_id"], "abc");
    }

    #[test]
    fn empty_block_yields_none() {
        assert!(parse_widget_sse_block("").is_none());
    }

    #[test]
    fn comment_alongside_event_does_not_demote_to_keepalive() {
        // Real events take precedence over a comment in the same block.
        let block = ": comment first\nevent: recording_state\ndata: {\"is_recording\":true}";
        let evt = parse_widget_sse_block(block).expect("event yields despite comment");
        assert_eq!(evt.name, "recording_state");
        assert_eq!(evt.payload["is_recording"], true);
    }

    #[test]
    fn unknown_field_is_ignored() {
        // `id:` and `retry:` aren't used; a block with only those is
        // structurally empty.
        let block = "id: 42\nretry: 1000";
        assert!(parse_widget_sse_block(block).is_none());
    }
}
