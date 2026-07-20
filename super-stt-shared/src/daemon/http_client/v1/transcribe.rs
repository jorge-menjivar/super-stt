// SPDX-License-Identifier: GPL-3.0-only
use super::super::internal::error::HttpResult;
use super::super::internal::sse;
use super::super::internal::transport;
use crate::models::protocol::DaemonResponse;
use std::path::PathBuf;

/// Options for [`transcribe`]. v1 only wires the daemon-mic capture path
/// (no `audio_data`); pre-captured audio is handled by the daemon's
/// `POST /transcribe` when a client sends a top-level `audio_data` array.
#[derive(Debug, Default, Clone)]
pub struct TranscribeOptions {
    pub write_mode: bool,
    pub stop_mode: Option<String>,
    /// Hold the connection open for the result. `true` → the daemon streams
    /// SSE and returns the final transcription; `false` → fire-and-forget, the
    /// daemon returns `202 {message:"Recording started"}` and records in the
    /// background (stop it via [`transcribe_stop`]).
    pub wait: bool,
    /// Stream incremental `event: preview` SSE frames before the final `done`
    /// (only meaningful with `wait: true`). Independent of write-mode typing.
    pub stream_realtime: bool,
}

/// Build the `POST /transcribe` request body from options. Mic-capture options
/// are sent at the top level (the daemon reads them from the request body).
fn record_body(opts: &TranscribeOptions) -> serde_json::Value {
    let mut data = serde_json::json!({
        "write_mode":      opts.write_mode,
        "wait":            opts.wait,
        "stream_realtime": opts.stream_realtime,
    });
    if let Some(mode) = &opts.stop_mode {
        data["stop_mode"] = serde_json::Value::String(mode.clone());
    }
    data
}

/// One event from the `/transcribe` Server-Sent Events stream.
///
/// Matches the daemon's wire-level event types — `preview` for
/// in-flight text, `done` for the final transcription, `error` for a
/// daemon-side failure — each an SSE block of `event:` / `data:` lines.
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
/// variant: collapses the entire SSE event stream into a single
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
    // Fire-and-forget: the daemon returns a single `202` JSON ack, not an SSE
    // stream, so parse the response directly instead of reading events.
    if !opts.wait {
        let req = transport::build_post_json("/transcribe", &record_body(&opts), Some(token))?;
        return transport::send_request::<DaemonResponse>(&socket_path, req).await;
    }
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
    let req = transport::build_post_json("/transcribe", &record_body(&opts), Some(token))?;

    let response = transport::open(&socket_path, req, Some(transport::REQUEST_TIMEOUT)).await?;

    let status = response.status();
    if !status.is_success() {
        // Non-2xx response (e.g. 409 `recording_in_progress`, 403
        // `scope_denied`, 429 `rate_limited`). The body is the JSON error
        // envelope, not SSE, so map it like every other non-2xx rather than
        // letting the caller read it as an event stream and report
        // "transcribe stream ended unexpectedly".
        let body = transport::collect_body(response).await?;
        return Err(transport::error_for_status(status, &body));
    }

    // Parse Server-Sent Events as they arrive: each event is a block of
    // `field: value\n` lines terminated by a blank line. The framing loop is
    // shared with `/events` via `sse::block_stream` (Tier 2 #8); here it maps
    // each block to a typed `TranscribeEvent`.
    Ok(sse::block_stream(
        response.into_body(),
        parse_sse_block,
        TranscribeEvent::Error,
    ))
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
