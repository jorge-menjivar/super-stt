// SPDX-License-Identifier: GPL-3.0-only
use super::super::internal::error::{HttpError, HttpResult};
use super::super::internal::sse;
use super::super::internal::transport;
use http_body_util::Empty;
use hyper::{Method, Request};
use std::path::PathBuf;

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
        return Err(HttpError::Other(
            "events_stream requires at least one topic".to_string(),
        ));
    }
    let req = build_events_request(token, topics)?;
    // Machine-to-machine: bound the wait for response headers so a wedged
    // daemon surfaces a Disconnected (and the subscription reconnects)
    // instead of hanging the stream open forever.
    let response = transport::open(&socket_path, req, Some(transport::REQUEST_TIMEOUT)).await?;
    transport::check_subscribe_status(response)
        .await
        .map(parse_widget_event_stream)
}

fn build_events_request(
    token: &str,
    topics: &[&str],
) -> Result<Request<transport::RequestBody>, String> {
    let topics_csv = topics.join(",");
    Request::builder()
        .method(Method::GET)
        .uri(format!(
            "http://stt.local{}/events?topics={topics_csv}",
            transport::API_PREFIX
        ))
        .header("host", "stt.local")
        .header("accept", "text/event-stream")
        .header("authorization", format!("Bearer {token}"))
        .body(transport::RequestBody::Empty(Empty::new()))
        .map_err(|e| format!("Failed to build request: {e}"))
}

/// Wrap an SSE response body in an async stream that yields one
/// [`WidgetEvent`] per `event:` block. Reuses [`sse::find_blank_line`] for
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
                while let Some(boundary) = sse::find_blank_line(&buffer) {
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
    let fields = sse::parse_fields(block);
    if let Some(name) = fields.event {
        let payload: serde_json::Value =
            serde_json::from_str(&fields.data).unwrap_or(serde_json::Value::Null);
        return Some(WidgetEvent {
            name: name.to_string(),
            payload,
        });
    }
    if fields.saw_comment {
        return Some(WidgetEvent {
            name: "keepalive".to_string(),
            payload: serde_json::Value::Null,
        });
    }
    None
}

#[cfg(test)]
mod tests {
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
