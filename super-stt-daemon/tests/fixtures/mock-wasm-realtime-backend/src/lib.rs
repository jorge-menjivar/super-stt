// SPDX-License-Identifier: GPL-3.0-only
//! Generic mock REALTIME WASM backend: a `wit-bindgen` component targeting the
//! `realtime-backend` world. It serves canned `/v1` over `wasi:http`
//! (incoming-handler) AND a canned `super-stt:realtime/ws-server` session — read
//! the consumer's `start` frame, emit one `preview` and one `done` frame, then
//! close. It contacts NO upstream (never calls the `ws` import or
//! `wasi:http/outgoing-handler`), so it needs no network and runs in hosted CI.
//!
//! The realtime analog of `mock-wasm-backend`: the daemon's
//! `tests/wasm_mock_realtime.rs` loads it through the real `WasmBackend` host to
//! exercise the realtime orchestration (`realtime_session` →
//! `ws-server.handle`) with no real backend, model, or network.
#![allow(clippy::doc_markdown)]

wit_bindgen::generate!({
    path: "wit",
    world: "realtime-backend",
    generate_all,
    // The bundled WASI 0.2.0 dep WITs gate a few interfaces behind `@unstable`
    // flags; wit-bindgen parses every file in `wit/`, so enable them to resolve.
    features: [
        "clocks-timezone",
    ],
});

use exports::super_stt::realtime::ws_server::Guest as WsServerGuest;
use exports::wasi::http::incoming_handler::Guest as HttpGuest;
use super_stt::realtime::ws::{ConsumerStream, WsError, WsFrame};
use wasi::http::types::{
    Fields, IncomingRequest, Method, OutgoingBody, OutgoingResponse, ResponseOutparam,
};

/// The fixed transcription the batch `/v1/transcribe` path returns; the
/// daemon's `wasm_mock_realtime.rs` pins it.
pub const MOCK_TRANSCRIPTION: &str = "mock transcription";
/// The fixed transcript the realtime `done` frame carries; pinned likewise.
pub const MOCK_REALTIME_TRANSCRIPTION: &str = "mock realtime transcription";

struct Component;

impl HttpGuest for Component {
    fn handle(request: IncomingRequest, outparam: ResponseOutparam) {
        let (status, body) = route(&request);
        send_response(outparam, status, &body);
    }
}

impl WsServerGuest for Component {
    /// Canned realtime session: wait for the consumer's first frame (the
    /// `start`), emit a `preview` then a `done` frame, and close. Never opens an
    /// upstream WebSocket, so no egress occurs.
    fn handle(_headers: Vec<(String, Vec<u8>)>, consumer: ConsumerStream) -> Result<(), WsError> {
        // Block for the consumer's start frame (the daemon's host recv awaits it).
        match consumer.recv() {
            Ok(WsFrame::Text(_) | WsFrame::Binary(_)) => {}
            Ok(WsFrame::Close(_)) | Err(_) => return Ok(()),
        }
        let _ = consumer.send_text(
            &serde_json::json!({ "type": "preview", "text": MOCK_REALTIME_TRANSCRIPTION })
                .to_string(),
        );
        let _ = consumer.send_text(
            &serde_json::json!({ "type": "done", "transcription": MOCK_REALTIME_TRANSCRIPTION })
                .to_string(),
        );
        let _ = consumer.close();
        Ok(())
    }
}

export!(Component);

/// Dispatch a `/v1` request to a canned response (mirrors `mock-wasm-backend`).
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
