// SPDX-License-Identifier: GPL-3.0-only
//! Deepgram speech-to-text backend as a `wasi:http` proxy component.
//!
//! Implements the Super STT `/v1` backend contract
//! (`docs/protocol/backend/contract.md`) by exporting
//! `wasi:http/incoming-handler` and dispatching on method + path. The
//! component is stateless: the daemon injects the API key as the
//! `x-stt-secret-deepgram_api_key` request header and the model in the
//! transcribe body, and the component forwards audio to the Deepgram
//! transcription API over `wasi:http/outgoing-handler`.

// Casts are intentional in audio/WAV encoding; doc lint trips on brand names.
#![allow(clippy::cast_possible_truncation, clippy::doc_markdown)]

use wasi::exports::http::incoming_handler::Guest;
use wasi::http::types::{
    Fields, IncomingBody, IncomingRequest, Method, OutgoingBody, OutgoingRequest,
    OutgoingResponse, ResponseOutparam, Scheme,
};
use wasi::io::streams::StreamError;

struct Component;

impl Guest for Component {
    fn handle(request: IncomingRequest, outparam: ResponseOutparam) {
        let (status, body) = route(&request);
        send_response(outparam, status, &body);
    }
}

wasi::http::proxy::export!(Component);

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
/// the audio body, forward to Deepgram, and return the transcription.
fn transcribe(request: &IncomingRequest) -> (u16, Vec<u8>) {
    let entries = request.headers().entries();
    let Some(api_key) = header(&entries, "x-stt-secret-deepgram_api_key") else {
        return (
            400,
            to_vec(&serde_json::json!({
                "status": "error",
                "message": "missing_secret_deepgram_api_key",
                "detail": "Deepgram API key not set. Add it in Settings \u{2192} Models \u{2192} Deepgram.",
            })),
        );
    };
    let base_url = header(&entries, "x-stt-option-base_url")
        .unwrap_or_else(|| "https://api.deepgram.com".to_string());
    let model = header(&entries, "x-stt-model").unwrap_or_else(|| "nova-3".to_string());

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
    let language = req
        .get("language")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    match call_deepgram(&base_url, &api_key, &model, language.as_deref(), &audio, sample_rate) {
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

/// Send the audio to Deepgram's listen API and return the text.
fn call_deepgram(
    base_url: &str,
    api_key: &str,
    model: &str,
    language: Option<&str>,
    audio: &[f32],
    sample_rate: u32,
) -> Result<String, String> {
    let wav = encode_wav(audio, sample_rate);
    let (scheme, authority) = parse_base(base_url);

    let path = if let Some(lang) = language {
        format!("/v1/listen?model={model}&smart_format=true&language={lang}")
    } else {
        format!("/v1/listen?model={model}&smart_format=true")
    };

    let headers = Fields::new();
    headers
        .append("authorization", format!("Token {api_key}").as_bytes())
        .map_err(|e| format!("header: {e:?}"))?;
    headers
        .append("content-type", b"audio/wav")
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
        .set_path_with_query(Some(&path))
        .map_err(|()| "set_path")?;

    let out_body = request.body().map_err(|()| "request_body")?;
    let future = wasi::http::outgoing_handler::handle(request, None)
        .map_err(|e| format!("handle: {e:?}"))?;
    write_all(&out_body, &wav)?;
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

    // Deepgram response: results.channels[0].alternatives[0].transcript
    let json: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("parse: {e}"))?;
    json["results"]["channels"]
        .get(0)
        .and_then(|ch| ch["alternatives"].get(0))
        .and_then(|alt| alt["transcript"].as_str())
        .map(String::from)
        .ok_or_else(|| "no_transcript_field".to_string())
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

/// Encode f32 samples as a 16-bit PCM mono WAV.
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
