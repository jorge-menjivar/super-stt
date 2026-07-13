// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, json_response};
use crate::daemon::http::internal::helpers::responses::recording_in_progress_response;
use crate::daemon::http::state::AppState;
// Only the wasm-backends realtime handler references the daemon type / bare
// `Response`; gated so the subprocess-only and no-backend builds stay warning-clean.
#[cfg(feature = "wasm-backends")]
use crate::daemon::types::SuperSTTDaemon;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
#[cfg(feature = "wasm-backends")]
use axum::response::Response;
use std::sync::Arc;
use super_stt_shared::models::protocol::DaemonResponse;

/// Abort a realtime session whose consumer has sent no frame for this long.
/// During an active session the client streams audio continuously, so an idle
/// gap this large means the connection is dead (half-open TCP, etc.). Aborting
/// drops the session future and releases the model read lock.
#[cfg(feature = "wasm-backends")]
const REALTIME_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(1);

/// `GET /v1/transcribe/realtime` — upgrade the connection and bridge it to the
/// active model's realtime session.
#[cfg(feature = "wasm-backends")]
pub(crate) async fn realtime_ws_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    let daemon = Arc::clone(&state.daemon);
    ws.on_upgrade(move |socket| run_realtime_session(socket, daemon))
}

/// Drive one realtime session: split the consumer socket, bridge each half to
/// the guest's consumer-stream channels, and invoke `realtime_session` while
/// holding the model read lock.
#[cfg(feature = "wasm-backends")]
async fn run_realtime_session(socket: axum::extract::ws::WebSocket, daemon: Arc<SuperSTTDaemon>) {
    use crate::stt_models::wasm::ws_host::{ConsumerStreamTransport, WsFrame};
    use axum::extract::ws::Message;
    use futures::{SinkExt, StreamExt};
    use tokio::sync::mpsc;

    // Channels bridging the consumer socket and the guest's consumer-stream.
    // incoming: consumer -> guest ; outgoing: guest -> consumer.
    let (incoming_tx, incoming_rx) = mpsc::unbounded_channel::<WsFrame>();
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<WsFrame>();
    let transport = ConsumerStreamTransport {
        incoming: incoming_rx,
        outgoing: outgoing_tx,
    };

    let (mut sink, mut stream) = socket.split();

    // Shared timestamp updated by relay_in every time a consumer frame arrives.
    // The idle watchdog below reads it to detect half-open TCP connections.
    let last_activity = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let last_activity_in = std::sync::Arc::clone(&last_activity);

    // Relay A: consumer socket -> incoming_tx (guest input). Ends when the
    // socket closes; dropping incoming_tx makes the guest's recv return Closed.
    let relay_in = tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            *last_activity_in.lock().unwrap() = std::time::Instant::now();
            let frame = match msg {
                Message::Text(t) => WsFrame::Text(t.to_string()),
                Message::Binary(b) => WsFrame::Binary(b.into()),
                Message::Close(_) => break,
                // Ping/Pong are answered by axum automatically; ignore them.
                Message::Ping(_) | Message::Pong(_) => continue,
            };
            if incoming_tx.send(frame).is_err() {
                break;
            }
        }
        // incoming_tx drops here, so the guest's recv sees `Closed`.
    });

    // Relay B: outgoing_rx (guest output) -> consumer socket. Ends when the
    // guest drops outgoing_tx (session over), flushing any final `done` frame.
    let relay_out = tokio::spawn(async move {
        while let Some(frame) = outgoing_rx.recv().await {
            let msg = match frame {
                WsFrame::Text(s) => Message::Text(s.into()),
                WsFrame::Binary(b) => Message::Binary(b.into()),
                WsFrame::Close(cf) => {
                    let _ = sink
                        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                            code: cf.code,
                            reason: cf.reason.into(),
                        })))
                        .await;
                    break;
                }
            };
            if sink.send(msg).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Run the session holding the model read lock for its whole duration.
    // An idle watchdog races against the session future: if no consumer frame
    // arrives for REALTIME_IDLE_TIMEOUT the watchdog wins, the select! drops
    // the session future, and the read lock is released — preventing a
    // half-open TCP connection from wedging model switches indefinitely.
    {
        let guard = daemon.model.read().await;
        match guard.as_ref() {
            Some(loaded) if loaded.definition.realtime => {
                let session = loaded.instance.realtime_session(transport);
                let idle_watchdog = async {
                    loop {
                        let idle = last_activity.lock().unwrap().elapsed();
                        if idle >= REALTIME_IDLE_TIMEOUT {
                            return;
                        }
                        tokio::time::sleep(REALTIME_IDLE_TIMEOUT.saturating_sub(idle)).await;
                    }
                };
                tokio::select! {
                    res = session => {
                        if let Err(e) = res {
                            log::warn!("realtime session ended with error: {e:#}");
                        }
                    }
                    () = idle_watchdog => {
                        log::info!(
                            "realtime session idle for {}s; aborting and releasing model lock",
                            REALTIME_IDLE_TIMEOUT.as_secs()
                        );
                        // Dropping `session` (via select! completing on this arm)
                        // aborts the in-flight guest call and releases the read lock.
                    }
                }
            }
            Some(_) => {
                drop(guard);
                log::debug!("realtime WS rejected: active model is not a realtime model");
            }
            None => {
                drop(guard);
                log::debug!("realtime WS rejected: no active model loaded");
            }
        }
    }

    // Session done: the guest dropped outgoing_tx (or we errored and dropped
    // `transport`), so relay_out finishes on its own. Make sure relay_in stops
    // even if the consumer is still sending.
    relay_in.abort();
    let _ = relay_out.await;
}

/// `POST /transcribe` — start a daemon-mic recording, streaming named
/// SSE events back as the recording progresses. Same pattern modern
/// LLM streaming APIs (`OpenAI`, Anthropic, Mistral chat, Google Gemini)
/// use: POST with options in the body, response is `text/event-stream`.
///
/// Wire shape:
///
/// ```text
/// event: preview
/// data: {"text":"hello"}
///
/// event: preview
/// data: {"text":"hello world"}
///
/// event: done
/// data: {"transcription":"hello world"}
/// ```
///
/// On daemon-side error a single `event: error\ndata: {"message":"..."}`
/// frame is emitted instead. The stream always ends with one of
/// `done` / `error` and the connection closes. Closing the
/// connection mid-stream cancels the recording (the daemon detects
/// the disconnect on its next write attempt).
/// Emit a single SSE `event: <name>\ndata: <json>\n\n` frame onto the
/// outbound stream. Returns `false` if the receiver is gone (client
/// disconnected). `serde_json::to_string` never embeds raw newlines, so
/// the JSON fits on the single `data:` line by construction.
pub(crate) fn emit_sse_event(
    tx: &tokio::sync::mpsc::UnboundedSender<Result<axum::body::Bytes, std::io::Error>>,
    event: &str,
    data: &serde_json::Value,
) -> bool {
    let mut bytes = format!("event: {event}\ndata: ").into_bytes();
    bytes.extend_from_slice(serde_json::to_string(data).unwrap_or_default().as_bytes());
    bytes.extend_from_slice(b"\n\n");
    tx.send(Ok(axum::body::Bytes::from(bytes))).is_ok()
}

/// Monotonic id identifying one `/transcribe` request's claim on the shared
/// preview slot, so a racing request clears only its own sender.
fn next_preview_slot_id() -> u64 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub(crate) async fn transcribe(
    State(s): State<AppState>,
    body: Option<axum::Json<serde_json::Value>>,
) -> impl IntoResponse {
    let data = body.map(|axum::Json(v)| v);
    let req = build_request("record", data);

    // Reject with `409 recording_in_progress` if a cycle is already
    // in progress. `/v1/transcribe` is "start a fresh recording" only —
    // toggle semantics live in the client (see
    // `docs/protocol/endpoints/v1/transcribe.md`). The client is
    // expected to consult `GET /v1/status::busy` and call
    // `/v1/transcribe/stop` to end an in-flight capture.
    if *s.daemon.busy.read().await {
        return recording_in_progress_response();
    }

    // mpsc channel that produces SSE byte chunks into the HTTP response body.
    let (line_tx, line_rx) =
        tokio::sync::mpsc::unbounded_channel::<Result<axum::body::Bytes, std::io::Error>>();

    // Initial `: stream-open` comment sent BEFORE the spawn returns
    // its response so hyper on the client side sees response bytes
    // immediately. Without this, a short silent recording finishes
    // before the first 5 s keep-alive arm fires, and the client's
    // hyper drops the connection on idle — which then cancels the
    // spawn task's `cmd_fut` and leaves the daemon stuck with
    // `busy=true` forever (`record_and_transcribe` never
    // reaches `finalize_recording_session`).
    let _ = line_tx.send(Ok(axum::body::Bytes::from_static(b": stream-open\n\n")));

    let daemon = Arc::clone(&s.daemon);
    tokio::spawn(async move {
        // Hook into the daemon's preview-text channel so each preview
        // update gets forwarded as an SSE `preview` event. Claim the shared slot
        // atomically: if another `/transcribe` already holds it we lost the
        // busy-check race (line 203 is a plain read), so bail with an error
        // frame rather than clobbering the winner's stream.
        let (preview_tx, mut preview_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let slot_id = next_preview_slot_id();
        {
            let mut slot = daemon.preview_text.write().await;
            if slot.is_some() {
                let _ = emit_sse_event(
                    &line_tx,
                    "error",
                    &serde_json::json!({ "message": "recording_in_progress" }),
                );
                return;
            }
            *slot = Some((slot_id, preview_tx));
        }

        let cmd_fut = daemon.handle_command(req);
        let mut cmd_fut = std::pin::pin!(cmd_fut);
        let mut final_response: Option<DaemonResponse> = None;
        let mut client_disconnected = false;

        // SSE keep-alive: write `: keepalive\n\n` every 2 s of
        // inactivity so the hyper client (and any intermediate proxy)
        // doesn't drop the connection during a silent recording or a
        // slow CPU transcription pass. Short cadence is intentional —
        // `/v1/transcribe` with preview disabled has long stretches
        // with zero events, and the CLI's hyper has been observed to
        // disconnect after ~4 s of body silence.
        let keepalive_interval = tokio::time::Duration::from_secs(2);

        // Phase 1: drive `cmd_fut` to completion. Forward preview
        // events to the client and emit periodic keep-alive comments.
        // Break out only if `cmd_fut` finishes OR the client closes
        // the connection — and in the disconnect case we MUST still
        // run `cmd_fut` to completion in Phase 2 below so the daemon
        // cleanup chain runs.
        loop {
            tokio::select! {
                biased;
                resp = &mut cmd_fut => {
                    final_response = Some(resp);
                    break;
                }
                preview = preview_rx.recv() => {
                    if let Some(text) = preview {
                        let payload = serde_json::json!({ "text": text });
                        if !emit_sse_event(&line_tx, "preview", &payload) {
                            client_disconnected = true;
                            break;
                        }
                    }
                    // preview_rx.recv() returning None during Phase 1
                    // would mean our sender was dropped, which we
                    // don't do until Phase 3. Fall through and keep
                    // looping; cmd_fut should resolve soon.
                }
                () = tokio::time::sleep(keepalive_interval) => {
                    let bytes = axum::body::Bytes::from_static(b": keepalive\n\n");
                    if line_tx.send(Ok(bytes)).is_err() {
                        client_disconnected = true;
                        break;
                    }
                }
            }
        }

        // Phase 2: if the client gave up before the recording
        // finished, signal stop AND await `cmd_fut` to completion.
        // Cancelling `cmd_fut` here (by letting the async block end)
        // would leave `record_and_transcribe` mid-execution — it
        // would never reach `finalize_recording_session`, and the
        // daemon would be stuck with `busy=true` /
        // `manual_stop_tx=Some(_)` until the next daemon restart.
        if client_disconnected {
            if let Some(tx) = daemon.manual_stop_tx.read().await.as_ref() {
                let _ = tx.send(());
            }
            final_response = Some(cmd_fut.await);
        }

        // Phase 3: clear our preview slot and emit the terminal event.
        // If the client is gone, the SSE sends are silent no-ops. Clear the slot
        // only if it is still ours — a racing request must not null a sender it
        // doesn't own.
        {
            let mut slot = daemon.preview_text.write().await;
            if slot.as_ref().is_some_and(|(id, _)| *id == slot_id) {
                *slot = None;
            }
        }
        if let Some(resp) = final_response {
            if resp.status == "success" {
                let payload = serde_json::json!({
                    "transcription": resp.transcription,
                });
                let _ = emit_sse_event(&line_tx, "done", &payload);
            } else {
                let payload = serde_json::json!({
                    "message": resp.message,
                });
                let _ = emit_sse_event(&line_tx, "error", &payload);
            }
        }
    });

    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(line_rx);
    let body = axum::body::Body::from_stream(stream);

    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-store")
        .header("x-accel-buffering", "no") // disable proxy buffering, just in case
        .body(body)
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("content-type", "application/json")],
                String::from("{\"status\":\"error\"}"),
            )
                .into_response()
        })
        .into_response()
}

pub(crate) async fn transcribe_stop(State(s): State<AppState>) -> impl IntoResponse {
    // Idempotent: nothing to stop. Direct response — going through
    // `handle_record_command` here would start a fresh recording
    // because that path is shared with `record`'s start case.
    if !*s.daemon.busy.read().await {
        let resp = DaemonResponse::success().with_message("No recording in progress".to_string());
        return json_response(&resp);
    }

    // Recording active — dispatch through `handle_record_command`'s
    // toggle branch, which produces one of the documented stop
    // messages:
    //   - "Recording stop signal sent"
    //   - "Manual stop not enabled in current mode"
    //   - "Transcription in progress, please wait"
    // (See `docs/protocol/endpoints/v1/transcribe/stop.md`.)
    let req = build_request("record", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

/// Client-scope transcription routes. The realtime WebSocket route is
/// registered only under the `wasm-backends` feature.
pub(crate) fn routes() -> axum::Router<crate::daemon::http::state::AppState> {
    let router = axum::Router::new()
        .route("/transcribe", axum::routing::post(transcribe))
        .route("/transcribe/stop", axum::routing::post(transcribe_stop));
    #[cfg(feature = "wasm-backends")]
    let router = router.route(
        "/transcribe/realtime",
        axum::routing::get(realtime_ws_handler),
    );
    router
}
