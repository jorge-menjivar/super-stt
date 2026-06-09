// SPDX-License-Identifier: GPL-3.0-only
use super::super::internal::error::{HttpError, HttpResult};
use super::super::internal::sse;
use super::super::internal::transport;
use crate::models::protocol::DaemonResponse;
use std::path::PathBuf;

/// Options for [`transcribe`]. v1 only wires the daemon-mic capture path
/// (no `audio_data`); pre-captured audio is a follow-up.
#[derive(Debug, Default, Clone)]
pub struct TranscribeOptions {
    pub write_mode: bool,
    pub stop_mode: Option<String>,
    pub wait: bool,
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
    let req = transport::build_post_json("/transcribe", &data, Some(token))?;

    let response = transport::open(&socket_path, req).await?;

    let status = response.status();
    if status == hyper::StatusCode::UNAUTHORIZED {
        let body = transport::collect_body(response).await?;
        return Err(HttpError::InvalidSession {
            reason: transport::parse_invalid_session_reason(&body),
        });
    }
    if !status.is_success() {
        // Non-2xx response (e.g. 409 `recording_in_progress`, 403
        // `scope_denied`, 429 `rate_limited`). The body is JSON, not
        // SSE — parse `{"message": "..."}` and surface it as
        // `HttpError::Other` so the caller doesn't try to read it as
        // an event stream and report "transcribe stream ended
        // unexpectedly".
        let body = transport::collect_body(response).await?;
        let message: String = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned))
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(HttpError::Other(message));
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
                while let Some(boundary) = sse::find_blank_line(&buffer) {
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

fn parse_sse_block(block: &str) -> Option<TranscribeEvent> {
    let fields = sse::parse_fields(block);
    let payload: serde_json::Value =
        serde_json::from_str(&fields.data).unwrap_or(serde_json::Value::Null);
    match fields.event {
        Some("preview") => Some(TranscribeEvent::Preview(
            payload
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        )),
        Some("done") => Some(TranscribeEvent::Done(
            payload
                .get("transcription")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        )),
        Some("error") => Some(TranscribeEvent::Error(
            payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("transcribe error")
                .to_string(),
        )),
        _ => None,
    }
}

/// `POST /transcribe/stop` — stop an in-flight daemon-mic recording.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable or the
/// response can't be parsed.
pub async fn transcribe_stop(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    let req = transport::build_post_json("/transcribe/stop", &serde_json::json!({}), Some(token))?;
    transport::send_request::<DaemonResponse>(&socket_path, req).await
}
