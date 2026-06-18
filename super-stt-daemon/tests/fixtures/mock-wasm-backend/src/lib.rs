// SPDX-License-Identifier: GPL-3.0-only
//! Generic mock WASM backend: a `wasi:http` proxy component that serves canned
//! `/v1` responses and makes no outbound calls. The daemon's `tests/wasm_mock.rs`
//! loads it through the real `WasmBackend` host to exercise the load → ping →
//! status → transcribe → teardown orchestration with no real backend, model, or
//! network — the WASM analog of `src/bin/mock_backend.rs`.

use wasi::exports::http::incoming_handler::Guest;
use wasi::http::types::{Fields, IncomingRequest, Method, OutgoingBody, OutgoingResponse, ResponseOutparam};

/// The fixed transcription the mock returns; assertions in `wasm_mock.rs` pin it.
pub const MOCK_TRANSCRIPTION: &str = "mock transcription";

struct Component;

impl Guest for Component {
    fn handle(request: IncomingRequest, outparam: ResponseOutparam) {
        let (status, body) = route(&request);
        send_response(outparam, status, &body);
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
        (Method::Post, "/v1/transcribe") => ok(&serde_json::json!({
            "status": "success", "transcription": MOCK_TRANSCRIPTION
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
