// SPDX-License-Identifier: GPL-3.0-only
//! Generic mock WASM backend: a `wasi:http` proxy component that serves canned
//! `/v1` responses and makes no outbound calls. The daemon's `tests/wasm_mock.rs`
//! loads it through the real `WasmBackend` host to exercise the load → ping →
//! status → synthesize → teardown orchestration with no real backend, model,
//! or network — the WASM analog of `src/bin/mock_backend.rs`.

use wasi::exports::http::incoming_handler::Guest;
use wasi::http::types::{Fields, IncomingRequest, Method, OutgoingBody, OutgoingResponse, ResponseOutparam};

/// Sample rate the mock declares on `/v1/synthesize`.
pub const MOCK_SAMPLE_RATE: u32 = 24000;
/// Number of PCM samples the mock synthesizes, split across several frames so
/// the test exercises reassembly rather than a single-frame happy path.
pub const MOCK_SAMPLE_COUNT: usize = 960;
/// How many audio frames the mock splits `MOCK_SAMPLE_COUNT` across.
pub const MOCK_AUDIO_FRAMES: usize = 4;

/// The known ramp the mock emits: sample `i` is `i` as an `i16`.
///
/// A ramp rather than silence or a constant, so a decoder that drops, reorders,
/// or misaligns bytes produces a visibly wrong sequence instead of a plausible
/// one.
#[must_use]
pub fn mock_samples() -> Vec<i16> {
    (0..MOCK_SAMPLE_COUNT)
        .map(|i| i16::try_from(i).unwrap_or(i16::MAX))
        .collect()
}

/// Frame kinds, mirroring `super_tts_shared::audio::frames::FrameKind`. Spelled
/// out rather than imported: this fixture builds for `wasm32-wasip2` and is
/// deliberately outside the workspace, so it shares no dependency with the
/// daemon — which also means it independently exercises the wire format
/// instead of round-tripping one implementation against itself.
const KIND_AUDIO: u8 = 0x01;
const KIND_MARK: u8 = 0x02;
const KIND_DONE: u8 = 0x03;

/// `[u8 kind][u32 len little-endian][payload]`.
fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// The full framed synthesis body: `MOCK_AUDIO_FRAMES` audio frames carrying
/// the ramp as s16le, one mark, then `done`.
fn synthesize_body() -> Vec<u8> {
    let samples = mock_samples();
    let pcm: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let per_frame = pcm.len() / MOCK_AUDIO_FRAMES;
    let mut out = Vec::new();
    for chunk in pcm.chunks(per_frame) {
        out.extend(frame(KIND_AUDIO, chunk));
    }
    out.extend(frame(
        KIND_MARK,
        br#"{"start_ms":0,"end_ms":40,"start_char":0,"end_char":5}"#,
    ));
    out.extend(frame(KIND_DONE, b"{}"));
    out
}

struct Component;

impl Guest for Component {
    fn handle(request: IncomingRequest, outparam: ResponseOutparam) {
        let method = request.method();
        let full = request.path_with_query().unwrap_or_default();
        let path = full.split('?').next().unwrap_or("");
        // Synthesis answers with the framed binary body and its own headers,
        // so it does not go through the JSON `route`.
        if matches!(method, Method::Post) && path == "/v1/synthesize" {
            send_response(
                outparam,
                200,
                &[
                    ("content-type", "application/vnd.super-tts.frames"),
                    ("x-tts-sample-rate", "24000"),
                    ("x-tts-channels", "1"),
                    ("x-tts-format", "s16le"),
                ],
                &synthesize_body(),
            );
            return;
        }
        let (status, body) = route(&request);
        send_response(
            outparam,
            status,
            &[("content-type", "application/json")],
            &body,
        );
    }
}

wasi::http::proxy::export!(Component);

/// Dispatch a `/v1` request to a canned response.
fn route(request: &IncomingRequest) -> (u16, Vec<u8>) {
    let method = request.method();
    let full = request.path_with_query().unwrap_or_default();
    let path = full.split('?').next().unwrap_or("");
    match (&method, path) {
        (Method::Get, "/v1/ping") => ok(&serde_json::json!({
            "status": "success", "message": "pong"
        })),
        (Method::Get, "/v1/status") => ok(&serde_json::json!({
            "status": "success", "state": "ready", "device": "remote"
        })),
        (Method::Post, "/v1/load") => (
            202,
            to_vec(&serde_json::json!({ "status": "success", "message": "Loading started" })),
        ),
        (Method::Post, "/v1/cancel") => ok(&serde_json::json!({
            "status": "success", "message": "Cancelled"
        })),
        _ => (
            404,
            to_vec(&serde_json::json!({ "status": "error", "message": "not_found" })),
        ),
    }
}

fn ok(value: &serde_json::Value) -> (u16, Vec<u8>) {
    (200, to_vec(value))
}

fn to_vec(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

/// Build the response and hand it to the outparam.
fn send_response(
    outparam: ResponseOutparam,
    status: u16,
    header_pairs: &[(&str, &str)],
    body_bytes: &[u8],
) {
    let headers = Fields::new();
    for (k, v) in header_pairs {
        let _ = headers.append(k, v.as_bytes());
    }
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
