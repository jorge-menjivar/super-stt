// SPDX-License-Identifier: GPL-3.0-only
//! Mistral speech-to-text backend targeting the `super-stt:realtime` world.
//!
//! The component exports both halves of that world:
//!
//! * `wasi:http/incoming-handler` — the Super STT `/v1` batch contract
//!   (`docs/protocol/backend/contract.md`), dispatching on method + path. This
//!   path is stateless: the daemon injects the API key as the
//!   `x-stt-secret-mistral_api_key` request header and the model in the
//!   transcribe body, and the component forwards audio to the Mistral
//!   transcription API over `wasi:http/outgoing-handler`.
//! * `super-stt:realtime/ws-server` — the realtime WebSocket session handler
//!   (see [`realtime`]). Currently a stub; the real Mistral upstream-WS bridge
//!   lands in a follow-up.

// Casts are intentional in audio/WAV encoding; doc lint trips on brand names.
#![allow(clippy::cast_possible_truncation, clippy::doc_markdown)]
// wit-bindgen's generated `Vec::from_raw_parts` glue trips this pedantic lint.
#![allow(clippy::same_length_and_capacity)]

wit_bindgen::generate!({
    path: "wit",
    world: "realtime-backend",
    generate_all,
    // The bundled WASI 0.2.0 dep WITs gate a handful of interfaces behind
    // `@unstable` feature flags (e.g. `wasi:clocks/timezone`). wit-bindgen
    // parses every file in `wit/`, so those gates must be enabled for the
    // directory to resolve even though `realtime-backend` does not use them.
    features: [
        "clocks-timezone",
    ],
});

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::{
    Fields, IncomingBody, IncomingRequest, Method, OutgoingBody, OutgoingRequest,
    OutgoingResponse, ResponseOutparam, Scheme,
};
use wasi::io::streams::StreamError;

mod realtime;

struct Component;

impl Guest for Component {
    fn handle(request: IncomingRequest, outparam: ResponseOutparam) {
        let (status, body) = route(&request);
        send_response(outparam, status, &body);
    }
}

export!(Component);

/// Dispatch a `/v1` request to its handler, returning `(status, json_bytes)`.
fn route(request: &IncomingRequest) -> (u16, Vec<u8>) {
    let method = request.method();
    let full = request.path_with_query().unwrap_or_default();
    let path = full.split('?').next().unwrap_or("");

    match (&method, path) {
        (Method::Get, "/v1/ping") => ok(&serde_json::json!({
            "status": "success", "message": "pong"
        })),
        (Method::Get, "/v1/status") => ok(&serde_json::json!({
            "status": "success", "state": "ready", "device": "cpu"
        })),
        (Method::Post, "/v1/load") => (
            202,
            to_vec(&serde_json::json!({ "status": "success", "message": "Loading started" })),
        ),
        (Method::Post, "/v1/cancel") => ok(&serde_json::json!({
            "status": "success", "message": "Cancelled"
        })),
        (Method::Post, "/v1/transcribe") => transcribe(request),
        _ => err(404, "not_found"),
    }
}

/// Handle `POST /v1/transcribe`: read the injected secret/option headers and
/// the audio body, forward to Mistral, and return the transcription.
fn transcribe(request: &IncomingRequest) -> (u16, Vec<u8>) {
    let entries = request.headers().entries();
    let Some(api_key) = header(&entries, "x-stt-secret-mistral_api_key") else {
        // Include a human-readable `detail` that the daemon surfaces to the user.
        return (
            400,
            to_vec(&serde_json::json!({
                "status": "error",
                "message": "missing_secret_mistral_api_key",
                "detail": "Mistral API key not set. Add it in Settings \u{2192} Models \u{2192} Mistral.",
            })),
        );
    };
    let base_url = header(&entries, "x-stt-option-base_url")
        .unwrap_or_else(|| "https://api.mistral.ai".to_string());
    let model =
        header(&entries, "x-stt-model").unwrap_or_else(|| "voxtral-mini-latest".to_string());

    let Ok(body) = request.consume() else {
        return err(400, "no_body");
    };
    let raw = match read_all(body) {
        Ok(r) => r,
        Err(e) => return err(500, &e),
    };
    let req: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(_) => return err(400, "invalid_json"),
    };

    let Some(audio) = req.get("audio_data").and_then(|v| v.as_array()) else {
        return err(400, "invalid_audio");
    };
    let audio: Vec<f32> = audio
        .iter()
        .map(|v| v.as_f64().unwrap_or(0.0) as f32)
        .collect();
    let sample_rate =
        u32::try_from(req.get("sample_rate").and_then(serde_json::Value::as_u64).unwrap_or(16000))
            .unwrap_or(16000);

    match call_mistral(&base_url, &api_key, &model, &audio, sample_rate) {
        Ok(text) => (
            200,
            to_vec(&serde_json::json!({ "status": "success", "transcription": text })),
        ),
        Err(detail) => (
            502,
            to_vec(&serde_json::json!({
                "status": "error", "message": "upstream_error", "detail": detail
            })),
        ),
    }
}

/// Send the audio to Mistral's transcription API and return the text.
fn call_mistral(
    base_url: &str,
    api_key: &str,
    model: &str,
    audio: &[f32],
    sample_rate: u32,
) -> Result<String, String> {
    let wav = encode_wav(audio, sample_rate);
    let boundary = "----superstt7MA4YWxkTrZu0gW";
    let multipart = build_multipart(boundary, model, &wav);
    let (scheme, authority) = parse_base(base_url);

    let headers = Fields::new();
    headers
        .append("authorization", format!("Bearer {api_key}").as_bytes())
        .map_err(|e| format!("header: {e:?}"))?;
    headers
        .append(
            "content-type",
            format!("multipart/form-data; boundary={boundary}").as_bytes(),
        )
        .map_err(|e| format!("header: {e:?}"))?;

    let request = OutgoingRequest::new(headers);
    request.set_method(&Method::Post).map_err(|()| "set_method")?;
    request
        .set_scheme(Some(&scheme))
        .map_err(|()| "set_scheme")?;
    request
        .set_authority(Some(&authority))
        .map_err(|()| "set_authority")?;
    request
        .set_path_with_query(Some("/v1/audio/transcriptions"))
        .map_err(|()| "set_path")?;

    // Obtain the body handle, start the request, then stream the body — the
    // canonical wasi:http outbound order.
    let out_body = request.body().map_err(|()| "request_body")?;
    let future = wasi::http::outgoing_handler::handle(request, None)
        .map_err(|e| format!("handle: {e:?}"))?;
    write_all(&out_body, &multipart)?;
    OutgoingBody::finish(out_body, None).map_err(|e| format!("finish: {e:?}"))?;

    let pollable = future.subscribe();
    pollable.block();
    let response = future
        .get()
        .ok_or("no_response")?
        .map_err(|()| "future_taken")?
        .map_err(|e| format!("http: {e:?}"))?;

    let status = response.status();
    let body = response.consume().map_err(|()| "response_consume")?;
    let bytes = read_all(body)?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "status {status}: {}",
            String::from_utf8_lossy(&bytes)
        ));
    }
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|e| format!("parse: {e}"))?
        .get("text")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| "no_text_field".to_string())
}

// ── helpers ───────────────────────────────────────────────────────────────

fn ok(value: &serde_json::Value) -> (u16, Vec<u8>) {
    (200, to_vec(value))
}

fn err(status: u16, message: &str) -> (u16, Vec<u8>) {
    (
        status,
        to_vec(&serde_json::json!({ "status": "error", "message": message })),
    )
}

fn to_vec(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

/// Case-insensitive header lookup over `Fields::entries()`.
fn header(entries: &[(String, Vec<u8>)], want: &str) -> Option<String> {
    entries
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(want))
        .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
}

/// Drain an incoming body to bytes.
fn read_all(body: IncomingBody) -> Result<Vec<u8>, String> {
    let stream = body.stream().map_err(|()| "no_stream".to_string())?;
    let mut out = Vec::new();
    loop {
        match stream.blocking_read(65536) {
            Ok(chunk) => out.extend_from_slice(&chunk),
            Err(StreamError::Closed) => break,
            Err(StreamError::LastOperationFailed(_)) => return Err("read_failed".to_string()),
        }
    }
    drop(stream);
    let _ = IncomingBody::finish(body);
    Ok(out)
}

/// Write all bytes to an outgoing body in ≤4096-byte flushes.
fn write_all(body: &OutgoingBody, data: &[u8]) -> Result<(), String> {
    let stream = body.write().map_err(|()| "write_stream".to_string())?;
    for chunk in data.chunks(4096) {
        stream
            .blocking_write_and_flush(chunk)
            .map_err(|_| "write_failed".to_string())?;
    }
    drop(stream);
    Ok(())
}

/// Build the response and hand it to the outparam.
fn send_response(outparam: ResponseOutparam, status: u16, body_bytes: &[u8]) {
    let headers = Fields::new();
    let _ = headers.append("content-type", b"application/json");
    let response = OutgoingResponse::new(headers);
    let _ = response.set_status_code(status);
    let Ok(body) = response.body() else {
        ResponseOutparam::set(outparam, Ok(response));
        return;
    };
    ResponseOutparam::set(outparam, Ok(response));
    if let Ok(stream) = body.write() {
        for chunk in body_bytes.chunks(4096) {
            let _ = stream.blocking_write_and_flush(chunk);
        }
        drop(stream);
    }
    let _ = OutgoingBody::finish(body, None);
}

/// Split a base URL into its scheme and authority (host[:port]).
fn parse_base(base: &str) -> (Scheme, String) {
    if let Some(rest) = base.strip_prefix("https://") {
        (Scheme::Https, rest.trim_end_matches('/').to_string())
    } else if let Some(rest) = base.strip_prefix("http://") {
        (Scheme::Http, rest.trim_end_matches('/').to_string())
    } else {
        (Scheme::Https, base.trim_end_matches('/').to_string())
    }
}

/// Encode f32 samples as a 16-bit PCM mono WAV (mirrors the daemon's
/// `encode_wav_in_memory`).
fn encode_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let bytes_per_sample: u32 = 2;
    let data_len = samples.len() as u32 * bytes_per_sample;
    let mut buf = Vec::with_capacity(44 + data_len as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // audio format = PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // channels = mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * bytes_per_sample).to_le_bytes()); // byte rate
    buf.extend_from_slice(&(bytes_per_sample as u16).to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        buf.extend_from_slice(&v.to_le_bytes());
    }
    buf
}

/// Build a `multipart/form-data` body with `model` and `file` (audio.wav).
fn build_multipart(boundary: &str, model: &str, wav: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"model\"\r\n\r\n");
    body.extend_from_slice(model.as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
    body.extend_from_slice(wav);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}
