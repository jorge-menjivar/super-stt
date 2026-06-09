// SPDX-License-Identifier: GPL-3.0-only
//! Mistral realtime WebSocket transcription bridge.
//!
//! Bridges a consumer WebSocket session to Mistral's realtime transcription
//! API (`wss://api.mistral.ai/v1/audio/transcriptions/realtime`).
//!
//! ## Half-duplex limitation
//! The host does not yet implement `wasi:io/poll` for the WS resources
//! (`subscribe` traps), so this guest cannot wait on the consumer and the
//! upstream at the same time. It therefore runs half-duplex: it forwards ALL
//! consumer audio to the upstream first, then drains the upstream's
//! transcript events. Mistral's incremental `delta` events buffer host-side
//! during the audio phase and are delivered to the consumer in the finalize
//! phase (so previews arrive in a burst near the end rather than live).
//! Implementing host-side `subscribe` would restore true streaming.

use base64::Engine as _;
use serde_json::{Value, json};

use crate::exports::super_stt::realtime::ws_server::Guest as WsServerGuest;
use crate::super_stt::realtime::ws::{self, ConsumerStream, WsError, WsFrame, WsStream};

const DEFAULT_BASE_URL: &str = "https://api.mistral.ai";
const DEFAULT_MODEL: &str = "voxtral-mini-transcribe-realtime-2602";
const COMMIT_JSON: &str = r#"{"type":"input_audio_buffer.commit"}"#;

impl WsServerGuest for crate::Component {
    fn handle(headers: Vec<(String, Vec<u8>)>, consumer: ConsumerStream) -> Result<(), WsError> {
        run(&headers, &consumer)
    }
}

fn run(headers: &[(String, Vec<u8>)], consumer: &ConsumerStream) -> Result<(), WsError> {
    let Some(api_key) = header(headers, "x-stt-secret-mistral_api_key") else {
        let _ = consumer.send_text(&error_json("missing mistral api key"));
        return Ok(());
    };
    let base_url =
        header(headers, "x-stt-option-base_url").unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let model = header(headers, "x-stt-model").unwrap_or_else(|| DEFAULT_MODEL.to_string());

    // 1. Read the consumer's `start` frame (sample_rate + optional language).
    let (sample_rate, language) = match consumer.recv()? {
        WsFrame::Text(s) => {
            let Ok(parsed) = parse_start(&s) else {
                let _ = consumer.send_text(&error_json("invalid start frame"));
                return Ok(());
            };
            parsed
        }
        WsFrame::Close(_) => return Ok(()), // consumer hung up before starting
        WsFrame::Binary(_) => {
            let _ = consumer.send_text(&error_json("audio before start frame"));
            return Ok(());
        }
    };

    // 2. Open the upstream WS and configure the session.
    let url = ws_url(&base_url, &model);
    let upstream = match ws::connect(
        &url,
        &[(
            "authorization".to_string(),
            format!("Bearer {api_key}").into_bytes(),
        )],
    ) {
        Ok(u) => u,
        Err(e) => {
            let _ = consumer.send_text(&error_json(&format!("upstream connect failed: {e:?}")));
            return Ok(());
        }
    };
    if let Err(e) = upstream.send_text(&session_update_json(sample_rate, language.as_deref())) {
        let _ = consumer.send_text(&error_json(&format!("session.update failed: {e:?}")));
        return Ok(());
    }

    // 3. PHASE 1 — forward all consumer audio to the upstream.
    loop {
        match consumer.recv()? {
            WsFrame::Binary(pcm) => {
                if let Err(e) = upstream.send_text(&audio_append_json(&pcm)) {
                    let _ = consumer.send_text(&error_json(&format!("upstream send failed: {e:?}")));
                    return Ok(());
                }
            }
            WsFrame::Text(s) if is_stop(&s) => break,
            WsFrame::Text(_) => {}      // ignore unknown control frames
            WsFrame::Close(_) => break, // consumer done; finalize what we have
        }
    }

    // 4. Commit and PHASE 2 — drain upstream transcript events.
    if let Err(e) = upstream.send_text(COMMIT_JSON) {
        let _ = consumer.send_text(&error_json(&format!("commit failed: {e:?}")));
        return Ok(());
    }
    drain_upstream(&upstream, consumer);
    let _ = consumer.close();
    Ok(())
}

/// PHASE 2 — read upstream transcript events until completion/close.
fn drain_upstream(upstream: &WsStream, consumer: &ConsumerStream) {
    let mut accumulated = String::new();
    loop {
        match upstream.recv() {
            Ok(WsFrame::Text(s)) => {
                if handle_upstream_event(&s, consumer, &mut accumulated) {
                    break; // completed
                }
            }
            Ok(WsFrame::Binary(_)) => {} // Mistral sends JSON text; ignore binary
            Ok(WsFrame::Close(_)) | Err(WsError::Closed) => {
                // Upstream closed without a completed event: emit what we have.
                let _ = consumer.send_text(&done_json(accumulated.trim()));
                break;
            }
            Err(e) => {
                let _ = consumer.send_text(&error_json(&format!("upstream recv failed: {e:?}")));
                break;
            }
        }
    }
}

// ── frame parsing ───────────────────────────────────────────────────────────

/// Parse the consumer's `start` frame: `{"type":"start","sample_rate":N,
/// "language":"xx"}`. Requires `type=="start"`; `sample_rate` defaults to
/// 16000; `language` is optional.
fn parse_start(s: &str) -> Result<(u32, Option<String>), WsError> {
    let v: Value =
        serde_json::from_str(s).map_err(|_| WsError::RecvFailed("invalid start frame".into()))?;
    if v.get("type").and_then(Value::as_str) != Some("start") {
        return Err(WsError::RecvFailed("invalid start frame".into()));
    }
    let sample_rate = v
        .get("sample_rate")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(16000);
    let language = v
        .get("language")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok((sample_rate, language))
}

/// `true` if `s` is a JSON object with `type=="stop"`.
fn is_stop(s: &str) -> bool {
    serde_json::from_str::<Value>(s)
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(Value::as_str)
                .map(|t| t == "stop")
        })
        .unwrap_or(false)
}

/// Handle one upstream JSON event. Returns `true` when the session is complete
/// (a `completed`/`error` event), `false` to keep draining.
fn handle_upstream_event(s: &str, consumer: &ConsumerStream, accumulated: &mut String) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(s) else {
        return false; // ignore non-JSON frames
    };
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");

    if kind == "error" {
        let msg = v
            .get("error")
            .and_then(|e| e.get("message"))
            .or_else(|| v.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("upstream error");
        let _ = consumer.send_text(&error_json(msg));
        return true;
    }

    if kind.ends_with("completed") {
        let transcript = v
            .get("transcript")
            .and_then(Value::as_str)
            .map_or_else(|| accumulated.trim().to_string(), str::to_string);
        let _ = consumer.send_text(&done_json(&transcript));
        return true;
    }

    if kind.ends_with("delta") {
        if let Some(delta) = v.get("delta").and_then(Value::as_str) {
            accumulated.push_str(delta);
            let _ = consumer.send_text(&preview_json(accumulated.trim()));
        }
        return false;
    }

    false // unknown event: ignore
}

// ── upstream URL + payloads ─────────────────────────────────────────────────

/// Convert an `http(s)://host` base URL into the realtime `ws(s)://` endpoint.
/// Defaults to `wss://` when no scheme is present.
fn ws_url(base_url: &str, model: &str) -> String {
    let host = if let Some(rest) = base_url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = base_url.strip_prefix("http://") {
        rest
    } else {
        base_url
    };
    let scheme = if base_url.starts_with("http://") {
        "ws"
    } else {
        "wss"
    };
    let host = host.trim_end_matches('/');
    format!("{scheme}://{host}/v1/audio/transcriptions/realtime?model={model}")
}

/// `session.update` payload configuring a PCM transcription session.
fn session_update_json(sample_rate: u32, language: Option<&str>) -> String {
    json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {
                "input": {
                    "format": { "type": "audio/pcm", "rate": sample_rate },
                    "transcription": { "language": language.unwrap_or("en") }
                }
            }
        }
    })
    .to_string()
}

/// `input_audio_buffer.append` payload carrying base64-standard PCM.
fn audio_append_json(pcm: &[u8]) -> String {
    let audio = base64::engine::general_purpose::STANDARD.encode(pcm);
    json!({ "type": "input_audio_buffer.append", "audio": audio }).to_string()
}

// ── consumer payloads ───────────────────────────────────────────────────────

fn preview_json(text: &str) -> String {
    json!({ "type": "preview", "text": text }).to_string()
}

fn done_json(text: &str) -> String {
    json!({ "type": "done", "transcription": text }).to_string()
}

fn error_json(msg: &str) -> String {
    json!({ "type": "error", "message": msg }).to_string()
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Case-insensitive header lookup over `&[(String, Vec<u8>)]`. Mirrors the
/// batch-path helper in `lib.rs`.
fn header(headers: &[(String, Vec<u8>)], want: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(want))
        .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
}
