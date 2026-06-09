# Realtime transcription via WebSocket — WASM end-to-end

**Date:** 2026-05-29
**Status:** Design — pending implementation
**Sibling specs:** None. App/applet/CLI integration with the consumer-facing WS endpoint is a follow-up spec.

## Goal

Extend the Super STT protocol and the wasm backend transport so realtime-only cloud models (the first being `voxtral-mini-transcribe-realtime-2602`) can ship as wasm backends. Realtime models speak WebSocket end-to-end: consumer ↔ daemon ↔ backend ↔ upstream. The current Mistral wasm backend gains a realtime model on top of its existing batch one.

## Non-goals

- Subprocess transport with allowlisted network egress. Rejected after the cross-platform sandboxing investigation — adds per-OS launcher work and a deprecated-API dependency on macOS to enforce what wasm sandboxes for free.
- Bidirectional streaming for batch models. Batch stays HTTP POST + optional SSE; realtime is the only WS-shaped path.
- Pseudo-realtime via chunked batch calls. Rejected as fake.
- WIT publishing infrastructure (e.g., GHCR via `wkg publish`). Deferred until backends actually split into separate repos. The canonical WIT lives in `docs/protocol/wit/`; backends bundle a copy.
- Helper crate wrapping the bindgen output. The raw bindgen API plus a `use` alias is ergonomic enough.

## Architecture

```
Consumer  <-WS->  Daemon  <-WS via WIT->  WASM backend  <-WS via WIT->  Upstream (Mistral)
```

Three transport hops, all WebSocket. The middle two use custom WIT interfaces the daemon implements as wasmtime host imports/exports. The wasm backend imports an outgoing-WS interface and exports an incoming-WS-server interface in addition to its existing `wasi:http/incoming-handler`.

## Custom WIT package: `super-stt:realtime@0.1.0`

Canonical location: `docs/protocol/wit/realtime.wit`. Bundled into each backend that needs it at `backends/<name>/wit/realtime.wit` (CI-enforced byte-identical). Documented for cross-language consumption in `docs/protocol/wit/README.md`.

```wit
package super-stt:realtime@0.1.0;

interface ws {
    /// Errors from WebSocket operations.
    variant ws-error {
        host-not-allowed(string),
        connect-failed(string),
        send-failed(string),
        recv-failed(string),
        invalid-url(string),
        closed,
    }

    record close-frame {
        code: u16,
        reason: string,
    }

    /// One frame read from or written to a WebSocket.
    variant ws-frame {
        text(string),
        binary(list<u8>),
        close(close-frame),
    }

    /// A live WebSocket connection. Drop closes it on the host side.
    resource ws-stream {
        send-text: func(text: string) -> result<_, ws-error>;
        send-binary: func(data: list<u8>) -> result<_, ws-error>;
        /// Blocking. Returns the next frame or an error. After a remote
        /// close, returns `ws-frame::close(...)` once, then `ws-error::closed`
        /// on subsequent calls.
        recv: func() -> result<ws-frame, ws-error>;
        /// Pollable for use with wasi:io/poll to multiplex with other streams.
        subscribe: func() -> pollable;
        close: func() -> result<_, ws-error>;
    }

    /// Open an outgoing WebSocket. The URL's host must appear in the
    /// backend's `[network].allowed_hosts`; the SSRF resolver rejects hosts
    /// resolving to loopback / link-local / private ranges. Scheme must be
    /// `ws://` or `wss://`.
    connect: func(
        url: string,
        headers: list<tuple<string, list<u8>>>,
    ) -> result<ws-stream, ws-error>;

    use wasi:io/poll@0.2.0.{pollable};
}

interface ws-server {
    use ws.{ws-frame, ws-error, close-frame};

    /// A live consumer WebSocket connection handed to the guest. Symmetric
    /// to `ws-stream` but the guest reads from / writes to the consumer
    /// rather than to an upstream service.
    resource consumer-stream {
        send-text: func(text: string) -> result<_, ws-error>;
        send-binary: func(data: list<u8>) -> result<_, ws-error>;
        recv: func() -> result<ws-frame, ws-error>;
        subscribe: func() -> pollable;
        close: func() -> result<_, ws-error>;
    }

    /// Daemon hands the guest a consumer connection plus the request-time
    /// headers (the daemon-injected `x-stt-*` headers — model name, secrets,
    /// options). Guest pumps frames between consumer and upstream and
    /// returns when the session ends.
    handle: func(
        headers: list<tuple<string, list<u8>>>,
        stream: consumer-stream,
    ) -> result<_, ws-error>;

    use wasi:io/poll@0.2.0.{pollable};
}

world realtime-backend {
    import wasi:http/outgoing-handler@0.2.0;
    import wasi:io/poll@0.2.0;
    import ws;
    export wasi:http/incoming-handler@0.2.0;
    export ws-server;
}
```

Two interfaces, both small. A backend that doesn't do realtime simply doesn't include `ws-server` in its world (uses `wasi:http/proxy` as today). A backend that does realtime exports `ws-server` and imports `ws`.

## Protocol changes

### `docs/protocol/backend/config.md`

`[[models]]` gains one optional field:

| Field | Type | Required | Notes |
|---|---|---|---|
| `realtime` | bool | no | Default `false`. When `true`, the model is reached over WebSocket end-to-end. The daemon routes consumer WS connections targeting this model to the backend's `ws-server` export; batch HTTP requests to such a model are rejected with `400 realtime_required`. |

`[capabilities]` table is new:

```toml
[capabilities]
    websocket = true
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `websocket` | bool | no | Default `false`. When `true`, the daemon wires the `super-stt:realtime/ws` import and validates the component exports `super-stt:realtime/ws-server`. Required for any backend that declares a model with `realtime = true`. A subprocess backend declaring `websocket = true` is rejected — the capability only applies to wasm transport. |

### `docs/protocol/backend/wasm.md`

New section describing the realtime WS contract:

- Components declaring `[capabilities] websocket = true` must:
  - Import `super-stt:realtime/ws@0.1.0` and `wasi:io/poll@0.2.0`.
  - Export `super-stt:realtime/ws-server@0.1.0`.
- Daemon enforcement for outbound WS: same allowlist + SSRF resolver as `wasi:http/outgoing-handler`. URL scheme must be `ws://` or `wss://`.
- `recv()` on either `ws-stream` or `consumer-stream` is blocking from the guest's perspective; the daemon parks the guest call on its async runtime.

### `docs/protocol/backend/contract.md`

New row in the request-headers table:

| Header | Carries |
|---|---|
| `x-stt-model-realtime` | `"true"` when the active model declares `realtime = true`. Absent otherwise. (Diagnostic — the backend already knows because the WS-server entry point is only called for realtime sessions.) |

New section on the consumer-facing WS endpoint:

- `GET /v1/transcribe/realtime` — opens a WebSocket session.
- Authentication: standard session token via `Authorization: Bearer` header on the WS handshake (mirrors the existing HTTP auth).
- Daemon-side flow:
  1. Validate session, look up active model.
  2. If model has `realtime = false`, reject with `400 not_realtime_model`.
  3. Instantiate the backend wasm component (or reuse the existing instance), invoke its `ws-server.handle()` export with the daemon's injected headers and a `consumer-stream` handle.
  4. Bridge frames between the consumer's WS and the wasm `consumer-stream` until either side closes.

WS frame protocol (consumer ↔ daemon):

| Direction | Frame type | Payload |
|---|---|---|
| Client → Server | text | `{"type":"start","sample_rate":16000,"language":"en"}` — first frame, configures the session. `language` is optional. |
| Client → Server | binary | Raw little-endian 16-bit PCM mono audio, exactly at `sample_rate`. |
| Client → Server | text | `{"type":"stop"}` — optional explicit end marker. WS close also implies stop. |
| Server → Client | text | `{"type":"preview","text":"hello wor"}` — incremental partial. |
| Server → Client | text | `{"type":"done","transcription":"hello world"}` — final result. Server closes after this. |
| Server → Client | text | `{"type":"error","message":"...","detail":"..."}` — fatal error. Server closes after this. |

### `docs/protocol/backend/subprocess.md`

No changes — realtime is wasm-only. Subprocess backends with realtime models are rejected at discovery time.

### `docs/protocol/endpoints/v1/transcribe.md`

Add a "Realtime models" section pointing at `/v1/transcribe/realtime` for those models, with the WS frame protocol above.

## Daemon implementation

### New module `super-stt-daemon/src/stt_models/wasm/ws_host.rs`

Implements both halves of the `super-stt:realtime` WIT:

- Generated via `wasmtime::component::bindgen!(path: "../docs/protocol/wit/realtime.wit", world: "realtime-backend")`.
- Outgoing `ws::connect`:
  - Parse URL, validate scheme ∈ `{ws, wss}`.
  - Hostname allowlist check against the backend's `[network].allowed_hosts` — same helper used by the existing `wasi:http/outgoing-handler`.
  - SSRF resolver — same as HTTP path; reject loopback/link-local/private.
  - `tokio_tungstenite::connect_async_tls_with_config` with the provided headers.
  - Wrap the returned `WebSocketStream` in a `WsStreamResource` and return as a wasmtime resource.
- `ws-stream` methods: thin wrappers over the underlying `WebSocketStream`. `recv` is async on the host side; wasmtime parks the synchronous guest call on its async runtime.
- `subscribe`: returns a `Pollable` so guests can `wasi:io/poll/poll(...)` on multiple streams.

### Changes in `super-stt-daemon/src/stt_models/wasm/mod.rs`

- `WasmBackend` constructor gains a `capabilities: WasmCapabilities` parameter wired from `backend.toml`'s `[capabilities]` table.
- When `capabilities.websocket` is true:
  - Add the `ws` host implementation to the `Linker`.
  - Validate the loaded component exports `ws-server` (fail discovery if missing).
- Add a `realtime_session(headers, consumer_stream)` method that invokes the guest's `ws-server.handle` export. This is what the new HTTP-WS handler in the daemon calls per session.

**Component lifecycle for realtime sessions:** the daemon instantiates a fresh wasm component instance per consumer WS session, mirroring the per-request pattern the existing `wasi:http/proxy` backends use. The instance lives for the duration of the consumer's WS connection; concurrent realtime sessions get independent state. This matches user expectation (one consumer = one transcription session) and avoids cross-session memory leaks. Instantiation cost is small (~1 ms for an already-loaded component).

### Consumer-facing WS endpoint in `super-stt-daemon/src/daemon/http_server.rs`

- New axum route: `get("/v1/transcribe/realtime", ws_handler)` using `axum::extract::ws`.
- Handler:
  1. Session auth (existing middleware).
  2. Read active backend + model from the daemon's state. If model isn't realtime, close with `1003 not_realtime_model`.
  3. Build the `x-stt-*` injected headers the backend expects (model, secrets, options).
  4. Create a `ConsumerStreamResource` wrapping the axum `WebSocketStream`.
  5. Call `wasm_backend.realtime_session(headers, consumer_stream).await`.
  6. On return (clean or error), close the consumer WS appropriately.

### Manifest parser changes in `super-stt-daemon/src/stt_models/backends/manifest.rs`

```rust
#[derive(Deserialize, Debug, Default)]
pub struct Capabilities {
    #[serde(default)]
    pub websocket: bool,
}

#[derive(Deserialize, Debug)]
pub struct ModelManifest {
    // existing fields...
    #[serde(default)]
    pub realtime: bool,
}

#[derive(Deserialize, Debug)]
pub struct BackendManifest {
    // existing fields...
    #[serde(default)]
    pub capabilities: Capabilities,
}
```

Validation additions:
- A subprocess backend with `capabilities.websocket = true` → reject ("websocket is wasm-only").
- A backend declaring any model with `realtime = true` but `capabilities.websocket = false` → reject ("realtime models require capabilities.websocket = true").
- A wasm backend with `capabilities.websocket = true` but missing the `ws-server` export → fail at component instantiation (caught when the daemon attempts the export validation).

**Catalog response surface:** `GET /backends` and `GET /models` responses must include each model's `realtime` boolean so the app can render a "realtime" badge and route to the correct endpoint (`/v1/transcribe/realtime` WS vs. `/v1/transcribe` POST). This is a single new field in the existing JSON responses — no new endpoints.

### Cargo deps the daemon picks up

```
tokio-tungstenite = { version = "0.26", features = ["rustls-tls-native-roots"] }
url = "2"
axum = { version = "0.8", features = ["ws"] }  # add "ws" feature
```

## Backend changes: `backends/mistral/`

Extends the existing wasm Mistral backend in place. No new crate.

### `backends/mistral/backend.toml`

```toml
[backend]
    source     = "github.com/super-stt/mistral"
    name       = "Mistral"
    version    = "0.2.0"                    # bump
    kind       = "wasm"
    entrypoint = "mistral.wasm"
    contract   = "v1"

[network]
    allowed_hosts = ["api.mistral.ai"]

[capabilities]
    websocket = true                        # new

[[secrets]]
    name = "mistral_api_key"
    label = "Mistral API key"
    required = true

[[options]]
    name = "base_url"
    type = "string"
    default = "https://api.mistral.ai"

[[models]]
    name = "voxtral-mini-latest"
    provider = "mistral"
    multilingual = true
    primary_language = "en"
    supported_languages = ["en"]
    supported_devices = ["none"]
    # realtime defaults to false

[[models]]
    name = "voxtral-mini-transcribe-realtime-2602"
    provider = "mistral"
    multilingual = true
    primary_language = "en"
    supported_languages = ["en"]
    supported_devices = ["none"]
    realtime = true                         # new
```

### `backends/mistral/wit/realtime.wit`

Byte-identical copy of `docs/protocol/wit/realtime.wit`. Created via `just sync-wit`; CI-enforced via `just check-wit-sync`.

### `backends/mistral/src/lib.rs`

Restructure: existing batch logic stays in `transcribe()`. New `ws-server.handle` export added. The world targeted by `wit_bindgen::generate!` changes from `wasi:http/proxy` to the new `realtime-backend` world.

Sketch:

```rust
wit_bindgen::generate!({
    path: "wit/realtime.wit",
    world: "realtime-backend",
});

use bindings::super_stt::realtime::ws::{self, WsFrame, WsStream};
use bindings::super_stt::realtime::ws_server::{ConsumerStream, Guest as WsServerGuest};
use bindings::wasi::io::poll;

struct Component;

// Existing wasi:http batch handler — unchanged in behavior, dispatches `/v1/transcribe` (batch only)
impl bindings::exports::wasi::http::incoming_handler::Guest for Component {
    fn handle(request: IncomingRequest, outparam: ResponseOutparam) {
        // existing batch routing
    }
}

// New realtime handler — invoked by the daemon when a consumer opens
// /v1/transcribe/realtime targeting a realtime model.
impl WsServerGuest for Component {
    fn handle(
        headers: Vec<(String, Vec<u8>)>,
        consumer: ConsumerStream,
    ) -> Result<(), ws::WsError> {
        realtime::run(headers, consumer)
    }
}

bindings::export!(Component with_types_in bindings);
```

`src/realtime.rs` is the new module — ~200 LoC pumping frames between the consumer and Mistral upstream. Skeleton:

```rust
pub fn run(
    headers: Vec<(String, Vec<u8>)>,
    consumer: ConsumerStream,
) -> Result<(), WsError> {
    let api_key = header(&headers, "x-stt-secret-mistral_api_key")
        .ok_or(WsError::SendFailed("missing api key".into()))?;
    let base_url = header(&headers, "x-stt-option-base_url")
        .unwrap_or_else(|| "https://api.mistral.ai".into());
    let model = header(&headers, "x-stt-model")
        .unwrap_or_else(|| "voxtral-mini-transcribe-realtime-2602".into());

    let ws_url = ws_url_from(&base_url, &model);
    let upstream = ws::connect(
        &ws_url,
        &[("authorization".into(), format!("Bearer {api_key}").into_bytes())],
    )?;

    // Send Mistral session.update.
    upstream.send_text(&session_update_json(&model))?;

    // Wait for the consumer's `start` frame (sample_rate + optional language).
    let session = wait_for_start_frame(&consumer)?;

    // Bidirectional pump. Poll both streams; act on whichever has a frame.
    let pollables = [consumer.subscribe(), upstream.subscribe()];
    let mut final_text = String::new();
    loop {
        let ready = poll::poll(&pollables);
        for &i in &ready {
            match i {
                0 => match consumer.recv()? {
                    WsFrame::Binary(audio) => {
                        upstream.send_text(&audio_append_json(&audio))?;
                    }
                    WsFrame::Text(s) if is_stop_message(&s) => {
                        upstream.send_text(&commit_json())?;
                    }
                    WsFrame::Close(_) => {
                        upstream.send_text(&commit_json())?;
                        // Continue draining upstream until it sends `completed`
                        return drain_upstream_until_done(&upstream, &consumer, &mut final_text);
                    }
                    _ => {}
                },
                1 => match upstream.recv()? {
                    WsFrame::Text(json) => {
                        handle_upstream_event(&json, &consumer, &mut final_text)?;
                    }
                    WsFrame::Close(_) => {
                        // Upstream closed unexpectedly.
                        let _ = consumer.send_text(r#"{"type":"error","message":"upstream_closed"}"#);
                        return Err(WsError::Closed);
                    }
                    _ => {}
                },
                _ => unreachable!(),
            }
        }
    }
}
```

The `handle_upstream_event` translates Mistral's `conversation.item.input_audio_transcription.delta`/`.completed`/`error` messages into the consumer-facing `preview`/`done`/`error` JSON frames.

### `backends/mistral/Cargo.toml`

```toml
[package]
    name = "super-stt-backend-mistral"
    version = "0.2.0"
    edition = "2024"

[lib]
    crate-type = ["cdylib"]

[dependencies]
serde_json = "1"
wit-bindgen = "0.30"            # WIT bindgen (replaces the wasi crate)
base64 = "0.22"                 # for audio_append JSON encoding
```

Note: the `wasi` crate dependency is replaced by `wit-bindgen` because we now generate from our custom WIT (which re-exports the wasi:http and wasi:io types).

## Sync workflow & CI

Add to the root `justfile`:

```make
# Re-copy the canonical WIT into every backend that bundles it.
sync-wit:
    for d in backends/*/wit; do \
        cp docs/protocol/wit/realtime.wit "$d/realtime.wit"; \
    done

# CI check: every bundled WIT must match the canonical.
check-wit-sync:
    for f in backends/*/wit/realtime.wit; do \
        diff -q "$f" docs/protocol/wit/realtime.wit \
            || { echo "WIT drift: $f"; exit 1; }; \
    done
```

Wire `check-wit-sync` into whatever lint/CI command runs in the existing pipeline. Workflow for protocol changes: edit `docs/protocol/wit/realtime.wit` → `just sync-wit` → `git add backends/*/wit/realtime.wit` → commit.

## Testing

### Unit-level: WIT host implementation

`super-stt-daemon/src/stt_models/wasm/ws_host.rs` ships with a small in-tree test that:
- Builds a stub wasm component (in a fixture dir) that exports `ws-server` and imports `ws`.
- The fixture's `ws-server.handle` immediately opens a `ws::connect` to a mock WebSocket server (using `tokio-tungstenite` as a server), echoes one frame, and returns.
- Test asserts: allowlist enforced, SSRF guard fires, happy path passes a frame end-to-end.

### Integration: end-to-end with mock upstream

`super-stt-daemon/tests/wasm_mistral_realtime.rs` mirrors the pattern of the existing `wasm_mistral.rs`:
- Spin up a mock WebSocket server (tokio-tungstenite server) emulating Mistral's `/v1/audio/transcriptions/realtime`.
- Open a consumer WS to the daemon's `/v1/transcribe/realtime` endpoint targeting `voxtral-mini-transcribe-realtime-2602`.
- Send `start` + a stream of binary PCM frames + close.
- Mock answers `session.created`, then a few `delta` events, then `completed`.
- Test asserts:
  - Bearer auth header reached the upstream.
  - Consumer received `preview` frames matching the deltas.
  - Consumer received `done` matching `completed`.
  - Allowlist enforcement: pointing the backend at a non-allowlisted upstream is rejected at `connect()`.
  - SSRF: pointing at `localhost` (resolvable to loopback) is rejected.

### Optional live test

`live_mistral_realtime` test, env-gated by `SUPER_STT_TEST_MISTRAL_REALTIME=1` + `MISTRAL_API_KEY`. Same pattern as the existing `live_mistral` / `live_openai` / `live_deepgram` tests. Streams a bundled WAV against the real API, asserts a non-empty transcription.

### Existing `wasm_mistral.rs`

Stays. Batch path is unchanged.

## Build & install

`backends/mistral/` is still wasm. The existing `build-mistral-backend` recipe in the `justfile` continues to work. `install-backends` continues to copy `mistral.wasm`. No subprocess machinery for this backend.

## Migration & versioning notes

- The Mistral backend's `version` bumps from `0.1.0` → `0.2.0` to reflect the protocol-relevant change (new model + new capability declaration).
- Users with the previous `0.1.0` installed: their installed `backend.toml` is replaced on the next `just install-backends`. The daemon will discover the new model on next restart.
- Custom (user-installed) backends with `realtime = true` declared but `capabilities.websocket = false` will be rejected at discovery with a clear error in the daemon log.

## Memory update post-implementation

`project_backend_only_architecture` currently states "Mistral (wasm, batch only — voxtral-mini-latest; the realtime model needs a WebSocket and is out of scope for `wasi:http`)". Update to:

> Mistral (wasm, batch + realtime — `voxtral-mini-latest` and `voxtral-mini-transcribe-realtime-2602`). Realtime models use the new `super-stt:realtime` WIT package: daemon implements `ws` (outgoing WS to upstream) and the wasm backend exports `ws-server` (incoming WS from the daemon). End-to-end WS shape: consumer ↔ daemon ↔ backend ↔ Mistral. Opt-in via `[capabilities] websocket = true` in `backend.toml`. WIT canonical at `docs/protocol/wit/`, vendored copy in each backend's `wit/` (CI-enforced sync via `just check-wit-sync`).

## Out of scope (follow-up specs)

1. **App / applet / CLI consumer integration.** None of the existing consumers know about `/v1/transcribe/realtime` yet. Adding WS-client + live-mic streaming is a separate sub-project with its own UI surface area.
2. **OpenAI realtime model.** OpenAI has a similar realtime API. Once this spec lands, adding it is mechanical (mirror Mistral's `realtime.rs` against OpenAI's slightly different event shapes).
3. **WIT distribution via OCI / GHCR.** Useful once backends start splitting into separate repos. ~5 lines of CI on tagged releases.
4. **`wasi-websockets` migration.** If/when the WASI websocket proposal stabilizes, replace our custom WIT with the standard one. Backend-author-facing API stays mostly the same.

## Validation checklist

- `cargo clippy --all-features --workspace -- -W clippy::pedantic -D warnings -D unused_must_use` passes.
- `just build-mistral-backend` produces a wasm component with both `wasi:http/incoming-handler` and `super-stt:realtime/ws-server` exports.
- `just install-backends` deposits the new `mistral.wasm`; daemon restart discovers both Mistral models (`voxtral-mini-latest` batch and `voxtral-mini-transcribe-realtime-2602` realtime).
- `cargo test --test wasm_mistral_realtime` passes against the mock upstream.
- `just check-wit-sync` passes (no WIT drift).
- A subprocess backend manifest declaring `[capabilities] websocket = true` is rejected with a clear error in the daemon log.
- A wasm backend declaring a realtime model without `[capabilities] websocket = true` is rejected.
- Live test (env-gated) against `api.mistral.ai` returns a non-empty transcription.
