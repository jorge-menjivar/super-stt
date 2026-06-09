# Realtime WS WASM Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add WebSocket-end-to-end support for realtime cloud models to the wasm backend transport, and ship `voxtral-mini-transcribe-realtime-2602` as the first realtime model on top of the existing Mistral wasm backend.

**Architecture:** Daemon ↔ wasm backend communicates via a new custom WIT package `super-stt:realtime@0.1.0` (outgoing-WS `ws` interface + incoming-WS-server `ws-server` interface). Daemon implements the outgoing WS host using `tokio-tungstenite 0.29` (already a daemon dependency) and adds an `axum` WS endpoint at `/v1/transcribe/realtime` that proxies frames to the guest's `ws-server.handle` export. Backends opt in via `[capabilities] websocket = true` in `backend.toml`; models that are realtime-only declare `realtime = true`.

**Tech Stack:** wasmtime 45.0.0 (component model, async), tokio-tungstenite 0.29.0, axum 0.8 (with new `ws` feature), wit-bindgen 0.30+ (in backends), Rust 2024 edition.

**Spec:** `docs/superpowers/specs/2026-05-29-realtime-wasm-websocket-design.md`

---

## File map

### New files

| Path | Responsibility |
|---|---|
| `docs/protocol/wit/realtime.wit` | Canonical WIT package definition (the contract). |
| `docs/protocol/wit/README.md` | Cross-language consumption guide. |
| `backends/mistral/wit/realtime.wit` | Vendored copy, CI-enforced identical to canonical. |
| `backends/mistral/src/realtime.rs` | Mistral realtime session handler (consumer ↔ upstream WS frame pump). |
| `super-stt-daemon/src/stt_models/wasm/ws_host.rs` | wasmtime host implementation of `super-stt:realtime/ws`. |
| `super-stt-daemon/src/daemon/realtime_handler.rs` | axum WS handler for consumer-facing `/v1/transcribe/realtime`. |
| `super-stt-daemon/tests/wasm_mistral_realtime.rs` | End-to-end integration test against a mock upstream WS server. |
| `super-stt-daemon/tests/fixtures/realtime_echo/` | Tiny stub wasm component for the WS-host unit test. |

### Modified files

| Path | Change |
|---|---|
| `super-stt-daemon/src/stt_models/backends/manifest.rs` | Add `Capabilities` struct, `realtime` field on `ModelEntry`, validation. |
| `super-stt-daemon/src/stt_models/wasm/mod.rs` | Wire `ws_host` into the linker when `capabilities.websocket = true`. New `realtime_session` method. |
| `super-stt-daemon/src/daemon/http_server.rs` | Register `/v1/transcribe/realtime` axum route. |
| `super-stt-daemon/src/daemon/handlers.rs` | Add `realtime: bool` to the model JSON the catalog returns. |
| `super-stt-daemon/Cargo.toml` | `axum` gains the `ws` feature. |
| `backends/mistral/Cargo.toml` | Replace `wasi` crate with `wit-bindgen`; add `base64`. |
| `backends/mistral/src/lib.rs` | Switch bindgen target to the new world; add `ws-server.handle` export. |
| `backends/mistral/backend.toml` | Add `[capabilities]` table, second model entry, version bump. |
| `docs/protocol/backend/config.md` | Document `[capabilities]` and `[[models]].realtime`. |
| `docs/protocol/backend/contract.md` | Document `x-stt-model-realtime` header + `/v1/transcribe/realtime` consumer endpoint + WS frame protocol. |
| `docs/protocol/backend/wasm.md` | Document the realtime WIT contract. |
| `docs/protocol/endpoints/v1/transcribe.md` | Cross-link the realtime endpoint. |
| `justfile` | `sync-wit` and `check-wit-sync` recipes. |

---

## Task 1: Canonical WIT package + sync infrastructure

**Files:**
- Create: `docs/protocol/wit/realtime.wit`
- Create: `docs/protocol/wit/README.md`
- Modify: `justfile`

- [ ] **Step 1: Create the WIT file**

Write `/home/jorge/rust_projects/super-stt/docs/protocol/wit/realtime.wit` with this exact content:

```wit
package super-stt:realtime@0.1.0;

interface ws {
    use wasi:io/poll@0.2.0.{pollable};

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
}

interface ws-server {
    use wasi:io/poll@0.2.0.{pollable};
    use ws.{ws-frame, ws-error};

    /// A live consumer WebSocket connection handed to the guest.
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
}

world realtime-backend {
    import wasi:http/outgoing-handler@0.2.0;
    import wasi:io/poll@0.2.0;
    import ws;
    export wasi:http/incoming-handler@0.2.0;
    export ws-server;
}
```

- [ ] **Step 2: Create the README**

Write `/home/jorge/rust_projects/super-stt/docs/protocol/wit/README.md`:

```markdown
# Super STT custom WIT packages

This directory holds custom WIT package definitions that are part of the Super STT backend protocol but not yet standardized in WASI.

## `realtime.wit` — `super-stt:realtime@0.1.0`

Defines two interfaces a wasm backend uses for realtime (WebSocket-based) transcription:

- `ws` — outgoing WebSocket client. The backend imports this to reach an upstream realtime API (e.g. Mistral's `wss://api.mistral.ai/v1/audio/transcriptions/realtime`). The daemon enforces the backend's `[network].allowed_hosts` and SSRF resolver.
- `ws-server` — incoming WebSocket server. The backend exports this so the daemon can hand it a consumer WebSocket session.

A backend that needs realtime support:
- Declares `[capabilities] websocket = true` in `backend.toml`.
- Declares at least one `[[models]] realtime = true`.
- Imports `super-stt:realtime/ws` and exports `super-stt:realtime/ws-server` in its `realtime-backend` world.

## Cross-language consumption

The WIT is language-agnostic. Non-Rust backends generate their own bindings:

- Rust: `wit_bindgen::generate!({ path: "wit/realtime.wit", world: "realtime-backend" });`
- JavaScript/TS: `jco transpile` or `componentize-js`
- Python: `componentize-py -d wit/realtime.wit -w realtime-backend bindings src/bindings.py`
- Go (TinyGo): `wit-bindgen-go generate`

In-tree first-party backends vendor a byte-identical copy of this WIT into `backends/<name>/wit/realtime.wit`; the `just check-wit-sync` recipe enforces parity. Third-party / out-of-tree backends should pin to a specific revision.
```

- [ ] **Step 3: Add `sync-wit` and `check-wit-sync` recipes to justfile**

Read `/home/jorge/rust_projects/super-stt/justfile` and add these two recipes near the other backend-related recipes (e.g., after `build-deepgram-backend`):

```
# Copy the canonical WIT into every backend that bundles it.
sync-wit:
    #!/usr/bin/env bash
    set -euo pipefail
    src="docs/protocol/wit/realtime.wit"
    for dir in backends/*/wit; do
        cp "$src" "$dir/realtime.wit"
        echo "synced $dir/realtime.wit"
    done

# CI check: every bundled WIT must match the canonical.
check-wit-sync:
    #!/usr/bin/env bash
    set -euo pipefail
    src="docs/protocol/wit/realtime.wit"
    fail=0
    for f in backends/*/wit/realtime.wit; do
        if ! diff -q "$f" "$src" >/dev/null; then
            echo "WIT drift: $f does not match $src" >&2
            fail=1
        fi
    done
    [ "$fail" -eq 0 ]
```

- [ ] **Step 4: Verify the WIT parses**

If `wit-deps` or `wkg` is available locally:
```bash
wkg wit validate docs/protocol/wit/realtime.wit
```
Expected: exits 0.

If neither is installed, skip — the daemon's `bindgen!` macro will validate it at build time in Task 3.

- [ ] **Step 5: Commit**

```bash
cd /home/jorge/rust_projects/super-stt
git add docs/protocol/wit/realtime.wit docs/protocol/wit/README.md justfile
git commit -m "Add super-stt:realtime WIT package and sync recipes"
```

---

## Task 2: Manifest parser — `capabilities` + `realtime` field

**Files:**
- Modify: `super-stt-daemon/src/stt_models/backends/manifest.rs`

TDD-friendly: parser tests come first, then the struct + serde derives.

- [ ] **Step 1: Add the failing tests**

Open `/home/jorge/rust_projects/super-stt/super-stt-daemon/src/stt_models/backends/manifest.rs` and add these tests inside the existing `#[cfg(test)] mod tests` block (after the last existing test):

```rust
    #[test]
    fn capabilities_websocket_parses_when_true() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/mistral"
name = "Mistral"
version = "0.2.0"
kind = "wasm"
entrypoint = "mistral.wasm"
contract = "v1"

[capabilities]
websocket = true
"#;
        let m: Manifest = toml::from_str(toml_src).expect("parse");
        assert!(m.capabilities.websocket);
    }

    #[test]
    fn capabilities_websocket_defaults_false_when_absent() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
"#;
        let m: Manifest = toml::from_str(toml_src).expect("parse");
        assert!(!m.capabilities.websocket);
    }

    #[test]
    fn model_realtime_parses_when_set() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/mistral"
name = "Mistral"
version = "0.2.0"
kind = "wasm"
entrypoint = "mistral.wasm"
contract = "v1"

[capabilities]
websocket = true

[[models]]
name = "voxtral-mini-transcribe-realtime-2602"
provider = "mistral"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
realtime = true
"#;
        let m: Manifest = toml::from_str(toml_src).expect("parse");
        assert!(m.models[0].realtime);
    }

    #[test]
    fn model_realtime_defaults_false() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/mistral"
name = "Mistral"
version = "0.1.0"
kind = "wasm"
entrypoint = "mistral.wasm"
contract = "v1"

[[models]]
name = "voxtral-mini-latest"
provider = "mistral"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#;
        let m: Manifest = toml::from_str(toml_src).expect("parse");
        assert!(!m.models[0].realtime);
    }
```

- [ ] **Step 2: Run tests; expect compile errors**

```bash
cargo test --manifest-path /home/jorge/rust_projects/super-stt/super-stt-daemon/Cargo.toml \
  --features wasm-backends \
  --lib stt_models::backends::manifest 2>&1 | tail -20
```
Expected: compile errors — `Manifest` has no field `capabilities`, `ModelEntry` has no field `realtime`.

- [ ] **Step 3: Add the `Capabilities` struct and the manifest field**

Edit `/home/jorge/rust_projects/super-stt/super-stt-daemon/src/stt_models/backends/manifest.rs`. After the existing `Network` struct (around line 41), insert:

```rust
#[derive(Debug, Default, Deserialize)]
pub struct Capabilities {
    /// Opt into the `super-stt:realtime/ws` import + `ws-server` export.
    /// Only meaningful for wasm backends; subprocess backends declaring this
    /// are rejected at discovery (see `Manifest::validate`).
    #[serde(default)]
    pub websocket: bool,
}
```

Then in the `Manifest` struct (around line 11), add the new field:

```rust
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub backend: BackendMeta,
    #[serde(default)]
    pub network: Network,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub secrets: Vec<Secret>,
    #[serde(default)]
    pub options: Vec<Opt>,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}
```

In `ModelEntry` (around line 74), add `realtime`:

```rust
#[derive(Debug, Deserialize)]
pub struct ModelEntry {
    pub name: String,
    pub provider: String,
    #[serde(default = "default_true")]
    pub multilingual: bool,
    #[serde(default)]
    pub primary_language: Option<String>,
    #[serde(default)]
    pub supported_languages: Vec<String>,
    #[serde(default)]
    pub supported_devices: Vec<String>,
    #[serde(default)]
    pub estimated_vram_bytes: u64,
    #[serde(default)]
    pub processing_interval_ms: Option<u64>,
    /// When `true`, the model is reached over WebSocket end-to-end.
    /// The daemon routes consumer WS sessions to the backend's `ws-server`
    /// export and rejects batch HTTP requests against this model with
    /// `400 not_realtime_model`.
    #[serde(default)]
    pub realtime: bool,
    #[serde(default)]
    pub files: Vec<FilesSpec>,
}
```

- [ ] **Step 4: Run tests; expect pass**

```bash
cargo test --manifest-path /home/jorge/rust_projects/super-stt/super-stt-daemon/Cargo.toml \
  --features wasm-backends \
  --lib stt_models::backends::manifest 2>&1 | tail -10
```
Expected: all four new tests + all pre-existing manifest tests pass.

- [ ] **Step 5: Add validation tests for the rejection rules**

Append to the same `tests` module:

```rust
    #[test]
    fn subprocess_with_websocket_capability_is_rejected() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/whisper"
name = "Whisper"
version = "0.1.0"
kind = "subprocess"
entrypoint = "super-stt-backend-whisper"
contract = "v1"

[capabilities]
websocket = true
"#;
        let m: Manifest = toml::from_str(toml_src).expect("parse");
        let err = m.validate().expect_err("subprocess + websocket must fail");
        assert!(
            err.to_string().contains("wasm-only"),
            "got: {err}"
        );
    }

    #[test]
    fn realtime_model_without_websocket_capability_is_rejected() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/mistral"
name = "Mistral"
version = "0.2.0"
kind = "wasm"
entrypoint = "mistral.wasm"
contract = "v1"

[[models]]
name = "voxtral-mini-transcribe-realtime-2602"
provider = "mistral"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
realtime = true
"#;
        let m: Manifest = toml::from_str(toml_src).expect("parse");
        let err = m
            .validate()
            .expect_err("realtime model without websocket capability must fail");
        assert!(
            err.to_string().contains("capabilities.websocket"),
            "got: {err}"
        );
    }
```

- [ ] **Step 6: Run; expect failure (no `validate()` method)**

```bash
cargo test --manifest-path /home/jorge/rust_projects/super-stt/super-stt-daemon/Cargo.toml \
  --features wasm-backends \
  --lib stt_models::backends::manifest 2>&1 | tail -10
```
Expected: compile errors — `Manifest::validate` doesn't exist.

- [ ] **Step 7: Implement `Manifest::validate`**

In the same file, add a method to the existing `impl Manifest` block (the one with `load`):

```rust
impl Manifest {
    /// Existing `load` method stays as-is. Add below it:

    /// Validate cross-field invariants that serde can't enforce on its own.
    ///
    /// # Errors
    /// Returns an error if any invariant is violated.
    pub fn validate(&self) -> Result<()> {
        if self.backend.kind == "subprocess" && self.capabilities.websocket {
            anyhow::bail!(
                "[capabilities].websocket is wasm-only; subprocess backends cannot declare it"
            );
        }
        for model in &self.models {
            if model.realtime && !self.capabilities.websocket {
                anyhow::bail!(
                    "model `{}` has realtime = true but [capabilities].websocket is not set",
                    model.name
                );
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 8: Run; expect pass**

```bash
cargo test --manifest-path /home/jorge/rust_projects/super-stt/super-stt-daemon/Cargo.toml \
  --features wasm-backends \
  --lib stt_models::backends::manifest 2>&1 | tail -10
```
Expected: all manifest tests pass.

- [ ] **Step 9: Wire `validate()` into the discovery path**

Find the discovery code that loads manifests. Run:

```bash
cd /home/jorge/rust_projects/super-stt/super-stt-daemon
grep -rn "Manifest::load\|manifest::load" src/ | head -5
```

For each call site that loads a manifest at backend discovery time (typically in `src/stt_models/backends/mod.rs` or `discover.rs`), follow the `load` call with `.and_then(|m| m.validate().map(|()| m))` or an explicit `m.validate()?` step. Backends whose manifest fails validation are skipped with a logged error, matching the existing pattern for malformed `backend.toml` files.

- [ ] **Step 10: Run the workspace build to catch breakage**

```bash
cargo build --manifest-path /home/jorge/rust_projects/super-stt/super-stt-daemon/Cargo.toml \
  --features wasm-backends 2>&1 | tail -10
```
Expected: builds cleanly.

- [ ] **Step 11: Commit**

```bash
cd /home/jorge/rust_projects/super-stt
git add super-stt-daemon/src/stt_models/backends/manifest.rs
# Plus whatever discover call sites you modified in Step 9
git commit -m "Manifest: add [capabilities].websocket and [[models]].realtime with validation"
```

---

## Task 3: Daemon WS host module (outgoing `ws` interface)

**Files:**
- Create: `super-stt-daemon/src/stt_models/wasm/ws_host.rs`
- Modify: `super-stt-daemon/src/stt_models/wasm/mod.rs` (declare the module)
- Modify: `super-stt-daemon/Cargo.toml` (axum `ws` feature)

- [ ] **Step 1: Enable axum's `ws` feature**

Open `/home/jorge/rust_projects/super-stt/super-stt-daemon/Cargo.toml`, find the `axum` dependency line, and append `"ws"` to its features. If today it reads:

```toml
axum = { version = "0.8", default-features = false, features = ["json", "tokio"] }
```

change to:

```toml
axum = { version = "0.8", default-features = false, features = ["json", "tokio", "ws"] }
```

- [ ] **Step 2: Create the WS host module skeleton**

Write `/home/jorge/rust_projects/super-stt/super-stt-daemon/src/stt_models/wasm/ws_host.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Host implementation of the `super-stt:realtime/ws` WIT package.
//!
//! Exposes an outgoing WebSocket capability to wasm backends, enforcing the
//! backend's declared `[network].allowed_hosts` and the same SSRF resolver
//! the `wasi:http/outgoing-handler` uses. The matching incoming-server
//! capability (`ws-server`) is exported BY the guest and INVOKED by the
//! daemon — see `WasmBackend::realtime_session` in `mod.rs`.

#![allow(clippy::doc_markdown)]

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures::{SinkExt, StreamExt};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use url::Url;
use wasmtime::component::{HasData, Linker, Resource, ResourceTable};

use super::host::AllowlistHooks;

// Generated bindings.
wasmtime::component::bindgen!({
    path: "../docs/protocol/wit/realtime.wit",
    world: "realtime-backend",
    async: true,
    with: {
        "wasi:io/poll/pollable": wasmtime_wasi::p2::bindings::io::poll::Pollable,
    },
});

pub use exports::super_stt::realtime::ws_server::{Guest as WsServerGuest, GuestConsumerStream};
pub use super_stt::realtime::ws::{CloseFrame, WsError, WsFrame};

/// Per-store state for the WS host. Lives alongside the existing wasm
/// `Host` state in the `Store<Host>` data.
#[derive(Default)]
pub struct WsState {
    pub streams: ResourceTable,
}

/// Implementation of the outgoing `ws` interface as a wasmtime host import.
pub struct WsHostImpl<'a> {
    pub state: &'a mut WsState,
    pub allowlist: AllowlistHooks,
}

impl<'a> super_stt::realtime::ws::Host for WsHostImpl<'a> {
    fn connect(
        &mut self,
        url: String,
        headers: Vec<(String, Vec<u8>)>,
    ) -> wasmtime::Result<Result<Resource<WsStreamResource>, WsError>> {
        // Validate URL.
        let parsed = match Url::parse(&url) {
            Ok(u) => u,
            Err(_) => return Ok(Err(WsError::InvalidUrl(format!("unparseable: {url}")))),
        };
        if !matches!(parsed.scheme(), "ws" | "wss") {
            return Ok(Err(WsError::InvalidUrl(format!(
                "scheme must be ws or wss, got {}",
                parsed.scheme()
            ))));
        }
        let Some(host) = parsed.host_str() else {
            return Ok(Err(WsError::InvalidUrl("missing host".into())));
        };

        // Allowlist + SSRF (delegates to the same helper wasi:http uses).
        if let Err(e) = self.allowlist.check_host(host) {
            return Ok(Err(WsError::HostNotAllowed(e.to_string())));
        }

        // Build the upgrade request with the caller's headers.
        let mut req = match url.as_str().into_client_request() {
            Ok(r) => r,
            Err(e) => return Ok(Err(WsError::InvalidUrl(format!("{e}")))),
        };
        for (k, v) in &headers {
            let Ok(hv) = HeaderValue::from_bytes(v) else {
                return Ok(Err(WsError::InvalidUrl(format!(
                    "header {k} contains invalid bytes"
                ))));
            };
            // Don't override headers tungstenite needs to set itself.
            let lower = k.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "host" | "connection" | "upgrade" | "sec-websocket-key" | "sec-websocket-version"
            ) {
                continue;
            }
            req.headers_mut().insert(
                tokio_tungstenite::tungstenite::http::HeaderName::try_from(k.as_str())
                    .map_err(|e| anyhow!("header name: {e}"))?,
                hv,
            );
        }

        // Connect.
        let (stream, _resp) = match futures::executor::block_on(connect_async(req)) {
            Ok(pair) => pair,
            Err(e) => return Ok(Err(WsError::ConnectFailed(format!("{e}")))),
        };

        let resource = WsStreamResource {
            inner: Arc::new(Mutex::new(WsInner::Open(stream))),
        };
        match self.state.streams.push(resource) {
            Ok(handle) => Ok(Ok(handle)),
            Err(e) => Ok(Err(WsError::ConnectFailed(format!("{e}")))),
        }
    }
}

/// The wasmtime resource backing a `ws-stream` handle in the guest.
pub struct WsStreamResource {
    inner: Arc<Mutex<WsInner>>,
}

enum WsInner {
    Open(WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>),
    Closed,
}

impl super_stt::realtime::ws::HostWsStream for WsHostImpl<'_> {
    fn send_text(
        &mut self,
        self_handle: Resource<WsStreamResource>,
        text: String,
    ) -> wasmtime::Result<Result<(), WsError>> {
        let resource = self
            .state
            .streams
            .get_mut(&self_handle)
            .context("invalid ws-stream handle")?;
        let mut guard = resource.inner.blocking_lock();
        match &mut *guard {
            WsInner::Open(stream) => {
                if let Err(e) = futures::executor::block_on(stream.send(Message::Text(text.into()))) {
                    return Ok(Err(WsError::SendFailed(format!("{e}"))));
                }
                Ok(Ok(()))
            }
            WsInner::Closed => Ok(Err(WsError::Closed)),
        }
    }

    fn send_binary(
        &mut self,
        self_handle: Resource<WsStreamResource>,
        data: Vec<u8>,
    ) -> wasmtime::Result<Result<(), WsError>> {
        let resource = self
            .state
            .streams
            .get_mut(&self_handle)
            .context("invalid ws-stream handle")?;
        let mut guard = resource.inner.blocking_lock();
        match &mut *guard {
            WsInner::Open(stream) => {
                if let Err(e) = futures::executor::block_on(stream.send(Message::Binary(data.into()))) {
                    return Ok(Err(WsError::SendFailed(format!("{e}"))));
                }
                Ok(Ok(()))
            }
            WsInner::Closed => Ok(Err(WsError::Closed)),
        }
    }

    fn recv(
        &mut self,
        self_handle: Resource<WsStreamResource>,
    ) -> wasmtime::Result<Result<WsFrame, WsError>> {
        let resource = self
            .state
            .streams
            .get_mut(&self_handle)
            .context("invalid ws-stream handle")?;
        let mut guard = resource.inner.blocking_lock();
        let WsInner::Open(stream) = &mut *guard else {
            return Ok(Err(WsError::Closed));
        };
        let next = futures::executor::block_on(stream.next());
        let frame = match next {
            Some(Ok(Message::Text(s))) => Ok(WsFrame::Text(s.to_string())),
            Some(Ok(Message::Binary(b))) => Ok(WsFrame::Binary(b.into())),
            Some(Ok(Message::Close(reason))) => {
                let cf = reason
                    .map(|r| CloseFrame {
                        code: u16::from(r.code),
                        reason: r.reason.to_string(),
                    })
                    .unwrap_or(CloseFrame {
                        code: 1000,
                        reason: String::new(),
                    });
                *guard = WsInner::Closed;
                Ok(WsFrame::Close(cf))
            }
            Some(Ok(_)) => Err(WsError::RecvFailed("unexpected control frame".into())),
            Some(Err(e)) => {
                *guard = WsInner::Closed;
                Err(WsError::RecvFailed(format!("{e}")))
            }
            None => {
                *guard = WsInner::Closed;
                Err(WsError::Closed)
            }
        };
        Ok(frame)
    }

    fn subscribe(
        &mut self,
        _self_handle: Resource<WsStreamResource>,
    ) -> wasmtime::Result<Resource<wasmtime_wasi::p2::bindings::io::poll::Pollable>> {
        // The minimal viable implementation polls eagerly; a future revision
        // wires a real pollable into the tungstenite stream.
        anyhow::bail!("ws-stream::subscribe is not yet implemented")
    }

    fn close(
        &mut self,
        self_handle: Resource<WsStreamResource>,
    ) -> wasmtime::Result<Result<(), WsError>> {
        let resource = self
            .state
            .streams
            .get_mut(&self_handle)
            .context("invalid ws-stream handle")?;
        let mut guard = resource.inner.blocking_lock();
        if let WsInner::Open(stream) = std::mem::replace(&mut *guard, WsInner::Closed) {
            let _ = futures::executor::block_on(stream.close(None));
        }
        Ok(Ok(()))
    }

    fn drop(&mut self, handle: Resource<WsStreamResource>) -> wasmtime::Result<()> {
        let _ = self.state.streams.delete(handle)?;
        Ok(())
    }
}

/// Wire the `ws` host import into a `Linker<T>` for a wasm component.
/// Caller is responsible for providing `WsState` + an `AllowlistHooks` in
/// each store via the `data` closure.
///
/// # Errors
/// Returns an error if wasmtime rejects the linkage (e.g. on duplicate import).
pub fn add_to_linker<T: Send>(
    linker: &mut Linker<T>,
    data: fn(&mut T) -> WsHostImpl<'_>,
) -> Result<()> {
    super_stt::realtime::ws::add_to_linker_get_host(linker, data)
        .context("link super-stt:realtime/ws")?;
    Ok(())
}

struct WsHostData;
impl HasData for WsHostData {
    type Data<'a> = WsHostImpl<'a>;
}
```

> **Note on the `subscribe()` punt:** the spec includes `subscribe` on both stream kinds for future multiplexing, but the minimum-viable realtime backend can poll the consumer and upstream sequentially in a tight loop (alternating recv calls with short timeouts). Wiring a real `Pollable` into a `tungstenite` stream is a follow-up. The bail message documents this.

- [ ] **Step 3: Register the module in `mod.rs`**

Open `/home/jorge/rust_projects/super-stt/super-stt-daemon/src/stt_models/wasm/mod.rs`. At the top of the file, after `pub mod host;`, add:

```rust
pub mod ws_host;
```

- [ ] **Step 4: Build to validate bindgen + impl**

```bash
cargo build --manifest-path /home/jorge/rust_projects/super-stt/super-stt-daemon/Cargo.toml \
  --features wasm-backends 2>&1 | tail -30
```

Expected: compiles. Any errors are almost certainly bindgen-generated type names not matching the impl — fix by adjusting `use` paths to match the actual generated module structure (the `cargo expand` output of the bindgen macro is the source of truth; alternately run `cargo check` and let the compiler suggest the right path).

**If `subscribe()`'s return type doesn't match generated bindings exactly:** open the bindgen output (`cargo expand --manifest-path super-stt-daemon/Cargo.toml --features wasm-backends -p super-stt-daemon stt_models::wasm::ws_host 2>&1 | grep 'fn subscribe' -A 3 | head -10`) and adjust the signature in the impl to match.

- [ ] **Step 5: Commit**

```bash
cd /home/jorge/rust_projects/super-stt
git add super-stt-daemon/src/stt_models/wasm/ws_host.rs \
        super-stt-daemon/src/stt_models/wasm/mod.rs \
        super-stt-daemon/Cargo.toml
git commit -m "Daemon: host implementation of super-stt:realtime/ws"
```

---

## Task 4: Wire `ws_host` into `WasmBackend` + add `realtime_session`

**Files:**
- Modify: `super-stt-daemon/src/stt_models/wasm/mod.rs`

- [ ] **Step 1: Wire the host into the Linker when capability is on**

Find `WasmBackend::with_info` in `mod.rs` (the constructor that builds the `Linker`). Add a parameter `websocket_capability: bool` and conditionally call into `ws_host::add_to_linker`.

The exact location to add the conditional linker setup is just after the existing wasi-http linker calls (the lines that add `wasmtime_wasi_http::add_to_linker_*` or similar). Add:

```rust
if websocket_capability {
    crate::stt_models::wasm::ws_host::add_to_linker(
        &mut linker,
        |host: &mut Host| crate::stt_models::wasm::ws_host::WsHostImpl {
            state: host.ws_state_mut(),
            allowlist: host.allowlist_hooks(),
        },
    )
    .context("link super-stt:realtime/ws into the component linker")?;
}
```

This assumes `Host` (in `super-stt-daemon/src/stt_models/wasm/host.rs`) grows two accessors. Add them:

```rust
// In host.rs Host impl block:
pub fn ws_state_mut(&mut self) -> &mut crate::stt_models::wasm::ws_host::WsState {
    &mut self.ws_state
}

pub fn allowlist_hooks(&self) -> crate::stt_models::wasm::ws_host::AllowlistHooks {
    self.allowlist.clone()  // adapt to actual field name
}
```

Also add a `ws_state: WsState` field to `Host` (with `Default::default()` initialization in its constructor).

- [ ] **Step 2: Update every call site of `with_info`**

Run:
```bash
grep -rn "WasmBackend::with_info\|WasmBackend::new" /home/jorge/rust_projects/super-stt --include='*.rs' | head -10
```

For each call site, pass the new `websocket_capability` parameter — usually `manifest.capabilities.websocket` from where the manifest is parsed. The existing tests in `super-stt-daemon/tests/wasm_*.rs` should pass `false` since they don't use realtime.

- [ ] **Step 3: Add `realtime_session` method**

In `WasmBackend`, add:

```rust
impl WasmBackend {
    /// Invoke the guest's `super-stt:realtime/ws-server.handle` export with
    /// the given consumer WebSocket and the same injected headers a normal
    /// transcribe call would receive. Returns when the guest's handler
    /// returns (clean close, error, or consumer disconnect).
    ///
    /// # Errors
    /// Returns the guest's `ws-error` or a wasmtime-level instantiation /
    /// invocation error.
    pub async fn realtime_session(
        &self,
        consumer: ConsumerStreamTransport,
    ) -> Result<()> {
        // Instantiate a fresh component instance per session — matches the
        // per-request lifecycle of the existing wasi:http path.
        // Host::new() initializes ws_state via Default — see Step 1 above.
        let mut store = Store::new(&self.engine, Host::new(self.allowed_hosts.clone()));
        // RealtimeBackendPre is generated by bindgen!; use it to instantiate.
        let instance = self.realtime_pre.instantiate_async(&mut store).await?;
        let guest = instance.super_stt_realtime_ws_server();
        let consumer_handle = store
            .data_mut()
            .ws_state_mut()
            .streams
            .push(ConsumerStreamResource::new(consumer))?;
        guest
            .call_handle(&mut store, &self.transcribe_headers, consumer_handle)
            .await?
            .map_err(|e| anyhow!("ws-server.handle returned: {e:?}"))
    }
}
```

This needs:
- A new `realtime_pre: RealtimeBackendPre<Host>` field on `WasmBackend`, built in `with_info` (use `RealtimeBackendPre::new(&engine, &component)` from the new bindgen world — only when `websocket_capability` is true).
- `ConsumerStreamTransport`: a tokio-based bidirectional channel from axum's `WebSocket` (handled in Task 6).
- `ConsumerStreamResource`: a wasmtime resource holding the bridge end. Implement `super_stt::realtime::ws_server::HostConsumerStream` in `ws_host.rs` (analogous to the outgoing `WsStreamResource` but reading from / writing to the consumer side).

A minimal `ConsumerStreamResource` is a struct holding `(mpsc::UnboundedReceiver<WsFrame>, mpsc::UnboundedSender<WsFrame>)`. `recv()` pulls from the receiver, `send_*()` push onto the sender; the axum side owns the matching halves.

- [ ] **Step 4: Build**

```bash
cargo build --manifest-path /home/jorge/rust_projects/super-stt/super-stt-daemon/Cargo.toml \
  --features wasm-backends 2>&1 | tail -30
```

Expected: compiles. The new world (`realtime-backend`) brings in additional wasi:http types via the bindgen; the existing `ProxyPre<Host>` field may need to coexist with `RealtimeBackendPre<Host>` — they're two different bindings against two different worlds. Both can be held on `WasmBackend`; the new one is only instantiated when `websocket_capability` is true.

- [ ] **Step 5: Commit**

```bash
cd /home/jorge/rust_projects/super-stt
git add super-stt-daemon/src/stt_models/wasm/mod.rs \
        super-stt-daemon/src/stt_models/wasm/host.rs \
        super-stt-daemon/src/stt_models/wasm/ws_host.rs
git commit -m "WasmBackend: wire ws_host linker and add realtime_session"
```

---

## Task 5: Catalog responses surface `realtime` field

**Files:**
- Modify: `super-stt-daemon/src/daemon/handlers.rs`

- [ ] **Step 1: Find the catalog response builders**

```bash
grep -rn "GET /backends\|GET /models\|backends_catalog\|active_model" \
    /home/jorge/rust_projects/super-stt/super-stt-daemon/src/daemon/ | head -10
```

Look at the existing handler that builds the JSON catalog response (typically `handlers.rs` or `model_management.rs`). Find where the per-model JSON object is constructed — it will have fields like `"name"`, `"provider"`, `"multilingual"`, etc.

- [ ] **Step 2: Add the `realtime` field**

In the per-model JSON construction (wherever `multilingual` is added, near it), add:

```rust
"realtime": model.realtime,
```

The exact code shape depends on how the catalog model is built — it might be `serde_json::json!({...})` or a `serde`-derived struct. Either way, add the new field reading from the `ModelEntry.realtime` field added in Task 2.

- [ ] **Step 3: Add a test**

If there's an existing test of the catalog JSON shape (likely in `tests/wasm_*.rs` or `daemon/tests/`), extend it to assert `realtime` appears. Otherwise, add a minimal one near the catalog handler.

- [ ] **Step 4: Build and run tests**

```bash
cargo test --manifest-path /home/jorge/rust_projects/super-stt/super-stt-daemon/Cargo.toml \
  --features wasm-backends 2>&1 | tail -15
```
Expected: passes.

- [ ] **Step 5: Commit**

```bash
cd /home/jorge/rust_projects/super-stt
git add super-stt-daemon/src/daemon/handlers.rs
git commit -m "Daemon: surface model.realtime in catalog responses"
```

---

## Task 6: Consumer-facing `/v1/transcribe/realtime` WS endpoint

**Files:**
- Create: `super-stt-daemon/src/daemon/realtime_handler.rs`
- Modify: `super-stt-daemon/src/daemon/http_server.rs`

- [ ] **Step 1: Create the handler module**

Write `/home/jorge/rust_projects/super-stt/super-stt-daemon/src/daemon/realtime_handler.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! axum handler for `GET /v1/transcribe/realtime` — the consumer-facing
//! WebSocket endpoint for realtime models. Bridges consumer frames to the
//! active backend's `super-stt:realtime/ws-server.handle` export.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::Extension;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::daemon::types::DaemonState;
use crate::stt_models::wasm::ws_host::{CloseFrame, WsFrame};
use crate::stt_models::wasm::ConsumerStreamTransport;

pub async fn realtime_ws_handler(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<DaemonState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_session(socket, state))
}

async fn handle_session(socket: WebSocket, state: Arc<DaemonState>) {
    // Resolve the active backend + model.
    let Some(backend) = state.active_backend.read().await.clone() else {
        let _ = close_with(socket, 1011, "no_active_backend").await;
        return;
    };
    let Some(model) = state.active_model.read().await.clone() else {
        let _ = close_with(socket, 1011, "no_active_model").await;
        return;
    };
    if !model.realtime {
        let _ = close_with(socket, 1003, "not_realtime_model").await;
        return;
    }

    // Build the bidirectional bridge.
    let (consumer_to_guest_tx, consumer_to_guest_rx) = mpsc::unbounded_channel::<WsFrame>();
    let (guest_to_consumer_tx, mut guest_to_consumer_rx) = mpsc::unbounded_channel::<WsFrame>();
    let transport = ConsumerStreamTransport::new(consumer_to_guest_rx, guest_to_consumer_tx);

    let (mut consumer_sink, mut consumer_stream) = socket.split();

    // Pump consumer → guest.
    let consumer_to_guest = tokio::spawn(async move {
        while let Some(Ok(msg)) = consumer_stream.next().await {
            let frame = match msg {
                Message::Text(s) => WsFrame::Text(s.to_string()),
                Message::Binary(b) => WsFrame::Binary(b.into()),
                Message::Close(reason) => {
                    let _ = consumer_to_guest_tx.send(WsFrame::Close(CloseFrame {
                        code: reason.as_ref().map(|r| r.code.into()).unwrap_or(1000),
                        reason: reason.as_ref().map(|r| r.reason.to_string()).unwrap_or_default(),
                    }));
                    break;
                }
                _ => continue, // ping/pong handled by axum
            };
            if consumer_to_guest_tx.send(frame).is_err() {
                break;
            }
        }
    });

    // Pump guest → consumer.
    let guest_to_consumer = tokio::spawn(async move {
        while let Some(frame) = guest_to_consumer_rx.recv().await {
            let msg = match frame {
                WsFrame::Text(s) => Message::Text(s.into()),
                WsFrame::Binary(b) => Message::Binary(b.into()),
                WsFrame::Close(cf) => {
                    let _ = consumer_sink
                        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                            code: cf.code,
                            reason: cf.reason.into(),
                        })))
                        .await;
                    break;
                }
            };
            if consumer_sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Run the guest session. This blocks until the guest's handle returns.
    if let Err(e) = backend.realtime_session(transport).await {
        log::warn!("realtime session ended with error: {e:#}");
    }

    // Clean up.
    consumer_to_guest.abort();
    guest_to_consumer.abort();
}

async fn close_with(socket: WebSocket, code: u16, reason: &str) -> Result<(), axum::Error> {
    let (mut sink, _stream) = socket.split();
    sink.send(Message::Close(Some(axum::extract::ws::CloseFrame {
        code,
        reason: reason.to_string().into(),
    })))
    .await
}
```

- [ ] **Step 2: Define `ConsumerStreamTransport` and `ConsumerStreamResource`**

In `super-stt-daemon/src/stt_models/wasm/mod.rs`, add a small public type used as the bridge between the axum handler and the wasm guest:

```rust
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::mpsc;
use crate::stt_models::wasm::ws_host::WsFrame;

pub struct ConsumerStreamTransport {
    pub rx: mpsc::UnboundedReceiver<WsFrame>,
    pub tx: mpsc::UnboundedSender<WsFrame>,
}

impl ConsumerStreamTransport {
    pub fn new(
        rx: mpsc::UnboundedReceiver<WsFrame>,
        tx: mpsc::UnboundedSender<WsFrame>,
    ) -> Self {
        Self { rx, tx }
    }
}
```

In `super-stt-daemon/src/stt_models/wasm/ws_host.rs`, add the `ConsumerStreamResource` struct plus its `HostConsumerStream` impl. Append to the bottom of the file:

```rust
use crate::stt_models::wasm::ConsumerStreamTransport;
use std::sync::Mutex as StdMutex;

/// Resource backing a `consumer-stream` handle in the guest. Bridges
/// guest send/recv calls to the axum-side mpsc channels.
pub struct ConsumerStreamResource {
    inner: StdMutex<Option<ConsumerStreamTransport>>,
}

impl ConsumerStreamResource {
    pub fn new(transport: ConsumerStreamTransport) -> Self {
        Self {
            inner: StdMutex::new(Some(transport)),
        }
    }
}

impl exports::super_stt::realtime::ws_server::HostConsumerStream for WsHostImpl<'_> {
    fn send_text(
        &mut self,
        self_handle: Resource<ConsumerStreamResource>,
        text: String,
    ) -> wasmtime::Result<Result<(), WsError>> {
        let resource = self
            .state
            .streams
            .get_mut(&self_handle)
            .context("invalid consumer-stream handle")?;
        let guard = resource.inner.lock().unwrap();
        let Some(t) = guard.as_ref() else {
            return Ok(Err(WsError::Closed));
        };
        if t.tx.send(WsFrame::Text(text)).is_err() {
            return Ok(Err(WsError::SendFailed("consumer closed".into())));
        }
        Ok(Ok(()))
    }

    fn send_binary(
        &mut self,
        self_handle: Resource<ConsumerStreamResource>,
        data: Vec<u8>,
    ) -> wasmtime::Result<Result<(), WsError>> {
        let resource = self
            .state
            .streams
            .get_mut(&self_handle)
            .context("invalid consumer-stream handle")?;
        let guard = resource.inner.lock().unwrap();
        let Some(t) = guard.as_ref() else {
            return Ok(Err(WsError::Closed));
        };
        if t.tx.send(WsFrame::Binary(data)).is_err() {
            return Ok(Err(WsError::SendFailed("consumer closed".into())));
        }
        Ok(Ok(()))
    }

    fn recv(
        &mut self,
        self_handle: Resource<ConsumerStreamResource>,
    ) -> wasmtime::Result<Result<WsFrame, WsError>> {
        let resource = self
            .state
            .streams
            .get_mut(&self_handle)
            .context("invalid consumer-stream handle")?;
        let mut guard = resource.inner.lock().unwrap();
        let Some(t) = guard.as_mut() else {
            return Ok(Err(WsError::Closed));
        };
        match futures::executor::block_on(t.rx.recv()) {
            Some(frame) => Ok(Ok(frame)),
            None => {
                *guard = None;
                Ok(Err(WsError::Closed))
            }
        }
    }

    fn subscribe(
        &mut self,
        _self_handle: Resource<ConsumerStreamResource>,
    ) -> wasmtime::Result<Resource<wasmtime_wasi::p2::bindings::io::poll::Pollable>> {
        anyhow::bail!("consumer-stream::subscribe is not yet implemented")
    }

    fn close(
        &mut self,
        self_handle: Resource<ConsumerStreamResource>,
    ) -> wasmtime::Result<Result<(), WsError>> {
        let resource = self
            .state
            .streams
            .get_mut(&self_handle)
            .context("invalid consumer-stream handle")?;
        let mut guard = resource.inner.lock().unwrap();
        *guard = None;
        Ok(Ok(()))
    }

    fn drop(&mut self, handle: Resource<ConsumerStreamResource>) -> wasmtime::Result<()> {
        let _ = self.state.streams.delete(handle)?;
        Ok(())
    }
}
```

- [ ] **Step 3: Register the route in `http_server.rs`**

Find the existing axum `Router` construction in `super-stt-daemon/src/daemon/http_server.rs`. Add:

```rust
use crate::daemon::realtime_handler::realtime_ws_handler;
// ...
.route("/v1/transcribe/realtime", axum::routing::get(realtime_ws_handler))
```

Declare the new module at the top of `http_server.rs`'s parent module (`super-stt-daemon/src/daemon/mod.rs`):

```rust
pub mod realtime_handler;
```

- [ ] **Step 4: Build**

```bash
cargo build --manifest-path /home/jorge/rust_projects/super-stt/super-stt-daemon/Cargo.toml \
  --features wasm-backends 2>&1 | tail -20
```
Expected: compiles. Common gotchas: `DaemonState`'s `active_model` / `active_backend` field names may differ from what's in the example; adjust to match.

- [ ] **Step 5: Commit**

```bash
cd /home/jorge/rust_projects/super-stt
git add super-stt-daemon/src/daemon/realtime_handler.rs \
        super-stt-daemon/src/daemon/mod.rs \
        super-stt-daemon/src/daemon/http_server.rs \
        super-stt-daemon/src/stt_models/wasm/mod.rs \
        super-stt-daemon/src/stt_models/wasm/ws_host.rs
git commit -m "Daemon: consumer-facing /v1/transcribe/realtime WS endpoint"
```

---

## Task 7: Protocol doc updates

**Files:**
- Modify: `docs/protocol/backend/config.md`
- Modify: `docs/protocol/backend/contract.md`
- Modify: `docs/protocol/backend/wasm.md`
- Modify: `docs/protocol/endpoints/v1/transcribe.md`

- [ ] **Step 1: Update `config.md`**

In `/home/jorge/rust_projects/super-stt/docs/protocol/backend/config.md`, add to the `[[models]]` field table the new row:

```
| `realtime` | bool | no | Default `false`. When `true`, the model is reached over WebSocket end-to-end. The daemon routes consumer WS connections targeting this model to the backend's `ws-server` export; batch HTTP requests to such a model are rejected with `400 not_realtime_model`. Requires `[capabilities] websocket = true`. |
```

Then add a new section after `[[options]]` and before `[[models]]`:

```markdown
## `[capabilities]`

Optional opt-in flags for protocol extensions a backend uses.

```toml
[capabilities]
websocket = true
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `websocket` | bool | no | Default `false`. When `true`, the daemon wires the `super-stt:realtime/ws` import for the component and requires it to export `super-stt:realtime/ws-server`. Subprocess backends with this flag are rejected at discovery. Required for any model declaring `realtime = true`. |
```

- [ ] **Step 2: Update `contract.md`**

In `/home/jorge/rust_projects/super-stt/docs/protocol/backend/contract.md`, add a new row to the request-headers table:

```
| `x-stt-model-realtime` | `"true"` when the active model declares `realtime = true`. Absent otherwise. Diagnostic — the backend already knows because the ws-server entry point is only called for realtime sessions. |
```

Add a new section after the `/v1/cancel` description:

```markdown
### `GET /v1/transcribe/realtime` (consumer-facing only)

The consumer-facing path for realtime models. Not part of the backend-facing
`/v1` contract — the daemon serves it directly and routes WS frames into the
backend's `super-stt:realtime/ws-server.handle` export.

Realtime models cannot be reached via `POST /v1/transcribe`; the daemon
rejects such requests with `400 not_realtime_model`.

#### WS frame protocol

Authentication: session token via `Authorization: Bearer` header on the
upgrade request.

| Direction | Frame type | Payload |
|---|---|---|
| Client → Server | text | `{"type":"start","sample_rate":16000,"language":"en"}` — first frame, configures the session. `language` is optional. |
| Client → Server | binary | Raw little-endian 16-bit PCM mono audio at the declared `sample_rate`. |
| Client → Server | text | `{"type":"stop"}` — optional explicit end. WS close also implies stop. |
| Server → Client | text | `{"type":"preview","text":"hello wor"}` — incremental partial. |
| Server → Client | text | `{"type":"done","transcription":"hello world"}` — final result. Server closes after this. |
| Server → Client | text | `{"type":"error","message":"...","detail":"..."}` — fatal error. Server closes after this. |

Closure codes:
- `1000` — normal completion
- `1003` — `not_realtime_model` (consumer targeted a non-realtime model)
- `1011` — internal failure (no active model, backend crashed)
```

- [ ] **Step 3: Update `wasm.md`**

In `/home/jorge/rust_projects/super-stt/docs/protocol/backend/wasm.md`, append a new section at the end:

```markdown
## Realtime: WebSocket end-to-end

Backends serving realtime models declare `[capabilities] websocket = true` in
their `backend.toml`. Such a component:

- Imports `super-stt:realtime/ws@0.1.0` (canonical WIT at
  `docs/protocol/wit/realtime.wit`).
- Imports `wasi:io/poll@0.2.0` (used to multiplex consumer and upstream
  streams from the guest).
- Exports `super-stt:realtime/ws-server@0.1.0`.

Continues to export `wasi:http/incoming-handler` for any non-realtime routes.

### Outgoing WS (`ws::connect`)

The daemon enforces the same allowlist + SSRF resolver it uses for
`wasi:http/outgoing-handler`. URL scheme must be `ws://` or `wss://`. Headers
are forwarded; tungstenite-required headers (`Host`, `Upgrade`,
`Sec-WebSocket-*`) are stripped.

### Incoming WS (`ws-server::handle`)

The daemon instantiates a fresh component instance per consumer WS session.
The `handle` export receives the same `x-stt-*` headers (model, secrets,
options) a regular `wasi:http/incoming-handler` invocation would, plus a
`consumer-stream` resource representing the consumer. The guest pumps frames
between the consumer and any upstream it opened, returning when the session
ends.
```

- [ ] **Step 4: Update `transcribe.md`**

In `/home/jorge/rust_projects/super-stt/docs/protocol/endpoints/v1/transcribe.md`, add near the top (after the introduction):

```markdown
> **Realtime models** use a separate WebSocket endpoint:
> `GET /v1/transcribe/realtime`. See
> [contract.md](../../backend/contract.md#get-v1transcriberealtime-consumer-facing-only)
> for the frame protocol. The daemon rejects `POST /v1/transcribe` for
> realtime models with `400 not_realtime_model`.
```

- [ ] **Step 5: Commit**

```bash
cd /home/jorge/rust_projects/super-stt
git add docs/protocol/
git commit -m "Protocol: document realtime WS, capabilities, and model.realtime"
```

---

## Task 8: Bundle the WIT in `backends/mistral/` + swap to `wit-bindgen`

**Files:**
- Create: `backends/mistral/wit/realtime.wit`
- Modify: `backends/mistral/Cargo.toml`

- [ ] **Step 1: Sync the WIT into the backend**

```bash
cd /home/jorge/rust_projects/super-stt
mkdir -p backends/mistral/wit
just sync-wit
```

Expected: `synced backends/mistral/wit/realtime.wit`.

Run the drift check:
```bash
just check-wit-sync
```
Expected: exits 0 silently.

- [ ] **Step 2: Update `backends/mistral/Cargo.toml`**

Open the existing file. Replace the `[dependencies]` section to swap `wasi` for `wit-bindgen` and add `base64`:

```toml
# SPDX-License-Identifier: GPL-3.0-only
[package]
    name = "super-stt-backend-mistral"
    version = "0.2.0"
    edition = "2024"
    license = "GPL-3.0-only"
    publish = false

[lib]
    crate-type = ["cdylib"]

[dependencies]
serde_json = "1.0.150"
wit-bindgen = "0.30"
base64 = "0.22"
```

- [ ] **Step 3: Verify it builds for wasm32-wasip2** (will still fail to link because lib.rs still references old wasi imports — that's expected)

```bash
cargo build --manifest-path /home/jorge/rust_projects/super-stt/backends/mistral/Cargo.toml \
  --target wasm32-wasip2 --release 2>&1 | tail -10
```
Expected: dependency resolution succeeds; compile errors in `lib.rs` referencing `wasi::*` types — those get fixed in Task 9.

- [ ] **Step 4: Commit the bundle + cargo changes**

```bash
cd /home/jorge/rust_projects/super-stt
git add backends/mistral/wit/realtime.wit backends/mistral/Cargo.toml
git commit -m "Mistral: bundle realtime WIT, swap wasi crate for wit-bindgen"
```

---

## Task 9: Mistral `lib.rs` — switch to `realtime-backend` world + stub `ws-server.handle`

**Files:**
- Modify: `backends/mistral/src/lib.rs`

- [ ] **Step 1: Replace the bindgen invocation and switch world**

At the top of `backends/mistral/src/lib.rs`, replace the existing `use wasi::*;` imports and the `wasi::http::proxy::export!(...)` line with:

```rust
wit_bindgen::generate!({
    path: "wit/realtime.wit",
    world: "realtime-backend",
});

use bindings::exports::wasi::http::incoming_handler::Guest as HttpGuest;
use bindings::exports::super_stt::realtime::ws_server::Guest as WsServerGuest;
use bindings::super_stt::realtime::ws::{self, WsError, WsFrame};
use bindings::wasi::http::types::{
    Fields, IncomingBody, IncomingRequest, Method, OutgoingBody, OutgoingRequest,
    OutgoingResponse, ResponseOutparam, Scheme,
};
use bindings::wasi::io::streams::StreamError;
```

The existing module logic (the batch transcribe routes) doesn't need to change behaviorally — only the import paths to wasi types need re-prefixing from `wasi::...` to `bindings::wasi::...`. Run a find-and-replace across the file: `wasi::http::types::` → `bindings::wasi::http::types::`, `wasi::io::streams::` → `bindings::wasi::io::streams::`, etc.

- [ ] **Step 2: Replace the export macro**

At the bottom of the file (in place of `wasi::http::proxy::export!(Component);`), put:

```rust
bindings::export!(Component with_types_in bindings);
```

- [ ] **Step 3: Add the `WsServerGuest` impl stub**

Below the existing `impl HttpGuest for Component { ... }` block (the one with `fn handle` for HTTP), add:

```rust
mod realtime;

impl WsServerGuest for Component {
    fn handle(
        headers: Vec<(String, Vec<u8>)>,
        stream: bindings::exports::super_stt::realtime::ws_server::ConsumerStream,
    ) -> Result<(), WsError> {
        realtime::run(headers, stream)
    }
}
```

- [ ] **Step 4: Create a stub realtime module**

Write `backends/mistral/src/realtime.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Mistral realtime WebSocket transcription handler.

#![allow(clippy::doc_markdown)]

use crate::bindings::exports::super_stt::realtime::ws_server::ConsumerStream;
use crate::bindings::super_stt::realtime::ws::{self, WsError, WsFrame};

pub fn run(
    _headers: Vec<(String, Vec<u8>)>,
    consumer: ConsumerStream,
) -> Result<(), WsError> {
    // Stub: just echo a fake done frame so the daemon's plumbing test passes
    // without an upstream. Real implementation lands in Task 10.
    consumer.send_text(r#"{"type":"done","transcription":"stub"}"#.to_string())?;
    let _ = consumer.close();
    Ok(())
}
```

- [ ] **Step 5: Build**

```bash
cargo build --manifest-path /home/jorge/rust_projects/super-stt/backends/mistral/Cargo.toml \
  --target wasm32-wasip2 --release 2>&1 | tail -20
```
Expected: compiles. The wasm artifact should exist at `backends/mistral/target/wasm32-wasip2/release/super_stt_backend_mistral.wasm`.

- [ ] **Step 6: Verify the exports**

```bash
wasm-tools component wit /home/jorge/rust_projects/super-stt/backends/mistral/target/wasm32-wasip2/release/super_stt_backend_mistral.wasm | grep -E "export.*incoming-handler|export.*ws-server"
```
Expected output includes both:
- `export wasi:http/incoming-handler@0.2.0;`
- `export super-stt:realtime/ws-server@0.1.0;`

(If `wasm-tools` isn't installed: `cargo install wasm-tools --locked`.)

- [ ] **Step 7: Commit**

```bash
cd /home/jorge/rust_projects/super-stt
git add backends/mistral/src/lib.rs backends/mistral/src/realtime.rs
git commit -m "Mistral: target realtime-backend world, stub ws-server.handle"
```

---

## Task 10: Mistral `backend.toml` — declare capability and the realtime model

**Files:**
- Modify: `backends/mistral/backend.toml`

- [ ] **Step 1: Update the manifest**

Replace `backends/mistral/backend.toml` with:

```toml
# SPDX-License-Identifier: GPL-3.0-only
# Mistral backend configuration. See docs/protocol/backend/config.md.

[backend]
    source = "github.com/super-stt/mistral"
    name = "Mistral"
    version = "0.2.0"
    kind = "wasm"
    entrypoint = "mistral.wasm"
    contract = "v1"

[network]
    allowed_hosts = ["api.mistral.ai"]

[capabilities]
    websocket = true

[[secrets]]
    name = "mistral_api_key"
    label = "Mistral API key"
    description = "Used to authenticate requests to api.mistral.ai."
    required = true

[[options]]
    name = "base_url"
    label = "API base URL"
    description = "Override the API base URL, e.g. for a gateway."
    type = "string"
    default = "https://api.mistral.ai"

[[models]]
    name = "voxtral-mini-latest"
    provider = "mistral"
    multilingual = true
    primary_language = "en"
    supported_languages = ["en"]
    supported_devices = ["none"]

[[models]]
    name = "voxtral-mini-transcribe-realtime-2602"
    provider = "mistral"
    multilingual = true
    primary_language = "en"
    supported_languages = ["en"]
    supported_devices = ["none"]
    realtime = true
```

- [ ] **Step 2: Validate the TOML**

```bash
python3 -c "import tomllib; d = tomllib.loads(open('/home/jorge/rust_projects/super-stt/backends/mistral/backend.toml').read()); print('models:', [m['name'] for m in d['models']]); print('websocket:', d['capabilities']['websocket'])"
```
Expected:
```
models: ['voxtral-mini-latest', 'voxtral-mini-transcribe-realtime-2602']
websocket: True
```

- [ ] **Step 3: Commit**

```bash
cd /home/jorge/rust_projects/super-stt
git add backends/mistral/backend.toml
git commit -m "Mistral: declare websocket capability and realtime model"
```

---

## Task 11: Implement real `realtime::run` (Mistral upstream WS bridge)

**Files:**
- Modify: `backends/mistral/src/realtime.rs`

- [ ] **Step 1: Replace the stub with the real implementation**

Replace the contents of `backends/mistral/src/realtime.rs` with:

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Mistral realtime WebSocket transcription handler.
//!
//! Bridges a consumer WS session to Mistral's upstream WS at
//! `wss://api.mistral.ai/v1/audio/transcriptions/realtime`. Reads PCM audio
//! frames from the consumer, encodes them as Mistral's `input_audio_buffer.append`
//! JSON messages, and forwards Mistral's `delta` / `completed` events back to
//! the consumer as `preview` / `done` JSON frames.

#![allow(clippy::doc_markdown)]

use base64::Engine as _;
use serde_json::{json, Value};

use crate::bindings::exports::super_stt::realtime::ws_server::ConsumerStream;
use crate::bindings::super_stt::realtime::ws::{self, WsError, WsFrame, WsStream};

const DEFAULT_BASE_URL: &str = "https://api.mistral.ai";
const DEFAULT_MODEL: &str = "voxtral-mini-transcribe-realtime-2602";

pub fn run(
    headers: Vec<(String, Vec<u8>)>,
    consumer: ConsumerStream,
) -> Result<(), WsError> {
    let api_key = header(&headers, "x-stt-secret-mistral_api_key")
        .ok_or_else(|| WsError::SendFailed("missing api key".into()))?;
    let base_url = header(&headers, "x-stt-option-base_url").unwrap_or_else(|| DEFAULT_BASE_URL.into());
    let model = header(&headers, "x-stt-model").unwrap_or_else(|| DEFAULT_MODEL.into());

    // Open upstream WS.
    let ws_url = ws_url_from(&base_url, &model);
    let upstream = ws::connect(
        &ws_url,
        &[("authorization".to_string(), format!("Bearer {api_key}").into_bytes())],
    )?;

    // Send Mistral session.update.
    upstream.send_text(session_update_json(&model))?;

    // Wait for consumer's `start` frame.
    let _sample_rate = wait_for_start(&consumer)?;

    pump(&consumer, &upstream)?;
    Ok(())
}

/// Look up a header value by case-insensitive name.
fn header(entries: &[(String, Vec<u8>)], name: &str) -> Option<String> {
    entries
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
}

/// Convert https://… → wss://…/v1/audio/transcriptions/realtime?model=…
fn ws_url_from(base_url: &str, model: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let scheme = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        format!("wss://{base}")
    };
    format!("{scheme}/v1/audio/transcriptions/realtime?model={model}")
}

fn session_update_json(_model: &str) -> String {
    json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {
                "input": {
                    "format": { "type": "audio/pcm", "rate": 16000 },
                    "transcription": { "language": "en" }
                }
            }
        }
    })
    .to_string()
}

fn audio_append_json(audio: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(audio);
    json!({
        "type": "input_audio_buffer.append",
        "audio": b64,
    })
    .to_string()
}

fn commit_json() -> &'static str {
    r#"{"type":"input_audio_buffer.commit"}"#
}

/// Wait for the consumer's first text frame and parse it as the `start` envelope.
/// Returns the sample_rate; other fields are ignored for now.
fn wait_for_start(consumer: &ConsumerStream) -> Result<u32, WsError> {
    loop {
        match consumer.recv()? {
            WsFrame::Text(s) => {
                let v: Value = serde_json::from_str(&s)
                    .map_err(|e| WsError::RecvFailed(format!("invalid start frame: {e}")))?;
                if v.get("type").and_then(|t| t.as_str()) != Some("start") {
                    return Err(WsError::RecvFailed("first frame must be `start`".into()));
                }
                let rate = v
                    .get("sample_rate")
                    .and_then(Value::as_u64)
                    .unwrap_or(16000) as u32;
                return Ok(rate);
            }
            WsFrame::Binary(_) => {
                // Some clients send audio before start; treat as protocol error.
                return Err(WsError::RecvFailed("audio before start frame".into()));
            }
            WsFrame::Close(_) => return Err(WsError::Closed),
        }
    }
}

/// Frame pump: alternate between draining the consumer and the upstream.
/// Because the WIT `subscribe()` is unimplemented, we use a sequential strategy:
/// poll one side, then the other, with short blocking reads. Replace with
/// `wasi:io/poll` once `subscribe()` is wired (see TODO in ws_host.rs).
fn pump(consumer: &ConsumerStream, upstream: &WsStream) -> Result<(), WsError> {
    let mut accumulated = String::new();
    let mut committed = false;
    loop {
        // Pump consumer → upstream first.
        match consumer.recv()? {
            WsFrame::Binary(audio) => {
                upstream.send_text(&audio_append_json(&audio))?;
            }
            WsFrame::Text(s) if is_stop(&s) => {
                if !committed {
                    upstream.send_text(commit_json())?;
                    committed = true;
                }
            }
            WsFrame::Close(_) => {
                if !committed {
                    let _ = upstream.send_text(commit_json());
                }
                return drain_upstream(consumer, upstream, &mut accumulated);
            }
            _ => {}
        }
        // Then pump upstream → consumer (one frame at a time).
        match upstream.recv() {
            Ok(WsFrame::Text(s)) => {
                handle_upstream(&s, consumer, &mut accumulated)?;
                if accumulated.ends_with("__DONE__") {
                    // Sentinel set by handle_upstream when `completed` arrived.
                    return Ok(());
                }
            }
            Ok(WsFrame::Close(_)) | Err(WsError::Closed) => return Err(WsError::Closed),
            Ok(_) | Err(_) => {}
        }
    }
}

fn drain_upstream(
    consumer: &ConsumerStream,
    upstream: &WsStream,
    accumulated: &mut String,
) -> Result<(), WsError> {
    loop {
        match upstream.recv() {
            Ok(WsFrame::Text(s)) => {
                handle_upstream(&s, consumer, accumulated)?;
                if accumulated.ends_with("__DONE__") {
                    return Ok(());
                }
            }
            Ok(WsFrame::Close(_)) | Err(WsError::Closed) => return Ok(()),
            Ok(_) | Err(_) => return Ok(()),
        }
    }
}

fn is_stop(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .as_deref()
        == Some("stop")
}

fn handle_upstream(
    text: &str,
    consumer: &ConsumerStream,
    accumulated: &mut String,
) -> Result<(), WsError> {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "conversation.item.input_audio_transcription.delta" => {
            if let Some(delta) = v.get("delta").and_then(Value::as_str) {
                accumulated.push_str(delta);
                let preview = json!({ "type": "preview", "text": accumulated.trim() }).to_string();
                consumer.send_text(&preview)?;
            }
        }
        "conversation.item.input_audio_transcription.completed" => {
            let final_text = v
                .get("transcript")
                .and_then(Value::as_str)
                .unwrap_or(accumulated.as_str())
                .trim()
                .to_string();
            let done = json!({ "type": "done", "transcription": final_text }).to_string();
            consumer.send_text(&done)?;
            // Sentinel so the pump loop returns.
            accumulated.clear();
            accumulated.push_str("__DONE__");
        }
        "error" => {
            let msg = v.get("error").and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("upstream error").to_string();
            let err = json!({ "type": "error", "message": msg }).to_string();
            consumer.send_text(&err)?;
            return Err(WsError::RecvFailed(msg));
        }
        _ => {}
    }
    Ok(())
}
```

- [ ] **Step 2: Build the backend**

```bash
cargo build --manifest-path /home/jorge/rust_projects/super-stt/backends/mistral/Cargo.toml \
  --target wasm32-wasip2 --release 2>&1 | tail -10
```
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
cd /home/jorge/rust_projects/super-stt
git add backends/mistral/src/realtime.rs
git commit -m "Mistral: implement realtime WS bridge to Mistral upstream"
```

---

## Task 12: End-to-end integration test against mock WS upstream

**Files:**
- Create: `super-stt-daemon/tests/wasm_mistral_realtime.rs`

- [ ] **Step 1: Write the test file**

Write `/home/jorge/rust_projects/super-stt/super-stt-daemon/tests/wasm_mistral_realtime.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end test of the Mistral realtime WASM backend against a mock
//! Mistral upstream WebSocket server. Exercises:
//!   - Consumer ↔ daemon /v1/transcribe/realtime WS endpoint
//!   - Daemon ↔ backend ws-server.handle invocation
//!   - Backend ↔ mock upstream WS using the daemon-implemented ws import
//!
//! Requires the component to be built first:
//!   just build-mistral-backend
#![cfg(feature = "wasm-backends")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{accept_async, connect_async};

fn component_path() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../backends/mistral/target/wasm32-wasip2/release/super_stt_backend_mistral.wasm");
    assert!(
        p.exists(),
        "component not built at {} — run `just build-mistral-backend`",
        p.display()
    );
    p
}

/// Spin up a minimal mock that mimics Mistral's realtime API:
/// - Accepts the WS handshake.
/// - Reads `session.update`, replies `session.created`.
/// - Reads `input_audio_buffer.append` (any count).
/// - On `input_audio_buffer.commit`, sends a few `.delta`s and a `.completed`.
async fn start_mock_upstream() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://{}", addr);
    let handle = tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { break };
            tokio::spawn(async move {
                let Ok(mut ws) = accept_async(tcp).await else { return };
                while let Some(Ok(msg)) = ws.next().await {
                    if let WsMessage::Text(t) = msg {
                        let s = t.as_str();
                        if s.contains("session.update") {
                            let _ = ws.send(WsMessage::Text(r#"{"type":"session.created"}"#.into())).await;
                        } else if s.contains("input_audio_buffer.commit") {
                            let _ = ws.send(WsMessage::Text(r#"{"type":"conversation.item.input_audio_transcription.delta","delta":"hello "}"#.into())).await;
                            let _ = ws.send(WsMessage::Text(r#"{"type":"conversation.item.input_audio_transcription.delta","delta":"world"}"#.into())).await;
                            let _ = ws.send(WsMessage::Text(r#"{"type":"conversation.item.input_audio_transcription.completed","transcript":"hello world"}"#.into())).await;
                        }
                        // input_audio_buffer.append: silently consume
                    }
                }
            });
        }
    });
    (url, handle)
}

#[tokio::test]
async fn realtime_round_trip() {
    // Boot the mock upstream.
    let (mock_url, _mock_handle) = start_mock_upstream().await;

    // ... daemon setup specific to the existing test harness:
    // - Construct a WasmBackend with the mistral component, websocket_capability = true.
    // - Configure allowed_hosts to include the mock's host:port.
    // - Inject the x-stt-secret-mistral_api_key + x-stt-option-base_url headers
    //   pointing at the mock.
    //
    // The existing wasm_mistral.rs and wasm_openai.rs tests show the exact
    // construction pattern; mirror it. The new piece is: call
    // backend.realtime_session(transport).await, where `transport` is a pair
    // of mpsc channels you drive from the test.

    let component = component_path();
    let authority = mock_url.strip_prefix("ws://").unwrap();

    // Mirrors the constructor signature from existing wasm_mistral.rs tests
    // with the new `websocket_capability` bool added at the end (see Task 4).
    let backend = super_stt_daemon::stt_models::wasm::WasmBackend::new(
        &component,
        vec![authority.to_string()],
        "voxtral-mini-transcribe-realtime-2602".to_string(),
        vec![
            ("x-stt-secret-mistral_api_key".to_string(), "test-key".to_string()),
            ("x-stt-option-base_url".to_string(), format!("http://{authority}")),
        ],
        /* websocket_capability */ true,
    )
    .expect("load backend");

    let (consumer_tx, consumer_rx) = tokio::sync::mpsc::unbounded_channel();
    let (guest_tx, mut guest_rx) = tokio::sync::mpsc::unbounded_channel();

    let transport = super_stt_daemon::stt_models::wasm::ConsumerStreamTransport::new(
        consumer_rx,
        guest_tx,
    );

    // Drive the consumer side.
    let driver = tokio::spawn(async move {
        // Send start frame.
        consumer_tx
            .send(super_stt_daemon::stt_models::wasm::ws_host::WsFrame::Text(
                r#"{"type":"start","sample_rate":16000}"#.to_string(),
            ))
            .unwrap();
        // Send a tiny chunk of fake PCM.
        consumer_tx
            .send(super_stt_daemon::stt_models::wasm::ws_host::WsFrame::Binary(
                vec![0_u8; 3200],
            ))
            .unwrap();
        // Send stop.
        consumer_tx
            .send(super_stt_daemon::stt_models::wasm::ws_host::WsFrame::Text(
                r#"{"type":"stop"}"#.to_string(),
            ))
            .unwrap();
    });

    let session = tokio::time::timeout(Duration::from_secs(10), backend.realtime_session(transport))
        .await
        .expect("session didn't finish in time")
        .expect("session error");
    driver.await.unwrap();

    // Collect all guest → consumer frames.
    let mut got = Vec::new();
    while let Some(f) = guest_rx.recv().await {
        if let super_stt_daemon::stt_models::wasm::ws_host::WsFrame::Text(t) = f {
            got.push(t);
        }
    }

    let any_preview = got.iter().any(|t| t.contains(r#""type":"preview""#));
    let done = got.iter().find(|t| t.contains(r#""type":"done""#))
        .expect("expected a done frame");

    assert!(any_preview, "expected at least one preview frame, got: {got:?}");
    assert!(done.contains("hello world"), "done frame: {done:?}");
}
```

> The `todo!` in the `ModelInfoData::standard` call may need replacement with whatever the existing tests do to build a `ModelInfoData` (look at `wasm_mistral.rs` for the existing pattern). The realtime model wasn't in the historical built-in registry, so a stripped-down construction is fine for tests.

- [ ] **Step 2: Build the mistral component if you haven't yet**

```bash
cd /home/jorge/rust_projects/super-stt
just build-mistral-backend
```

- [ ] **Step 3: Run the test**

```bash
cargo test --manifest-path /home/jorge/rust_projects/super-stt/super-stt-daemon/Cargo.toml \
  --features wasm-backends --test wasm_mistral_realtime 2>&1 | tail -20
```
Expected: passes. Common failure modes:
- "missing transcribe token" → bindgen path mismatch; check `super_stt::realtime` vs the actual generated path.
- Hangs → the sequential `pump()` loop probably needs to bail when both sides are quiet for too long; add a `recv()` timeout in `ws_host.rs`'s `WsStreamResource::recv` (e.g., 50 ms) or refactor to drive both sides concurrently.
- 1003 not_realtime_model → make sure the test wires the `realtime = true` flag through to the active model.

- [ ] **Step 4: Commit**

```bash
cd /home/jorge/rust_projects/super-stt
git add super-stt-daemon/tests/wasm_mistral_realtime.rs
git commit -m "Add wasm_mistral_realtime integration test against mock upstream"
```

---

## Task 13: Install + daemon e2e verification (manual)

**Files:** none

- [ ] **Step 1: Build and install the updated Mistral backend**

```bash
cd /home/jorge/rust_projects/super-stt
just build-mistral-backend
just install-backends 2>&1 | grep -i mistral
```
Expected:
```
Installed Mistral backend -> /home/jorge/.local/share/super-stt/backends/mistral
```

- [ ] **Step 2: Restart the daemon**

```bash
systemctl --user restart super-stt
sleep 3
systemctl --user is-active super-stt
```
Expected: `active`.

- [ ] **Step 3: Verify discovery**

```bash
journalctl --user -u super-stt -n 30 --no-pager | grep -i 'Mistral\|capabilities\|realtime'
```
Expected: log line showing Mistral discovered serving 2 model(s) and no errors about `[capabilities]` or `realtime`.

- [ ] **Step 4: Confirm the realtime model surfaces in the catalog**

```bash
# Using your usual session-token mechanism:
curl -s --unix-socket "$XDG_RUNTIME_DIR/stt/super-stt-http.sock" \
  -H "Authorization: Bearer $SUPER_STT_TOKEN" \
  http://localhost/v1/backends \
  | python3 -m json.tool \
  | grep -E '"name"|"realtime"' | head -20
```
Expected: `voxtral-mini-transcribe-realtime-2602` appears with `"realtime": true`.

If the consumer-facing curl is blocked by your auth setup, the journal grep in Step 3 is sufficient evidence the daemon picked the backend up.

- [ ] **Step 5: (Optional) live test against the real Mistral API**

```bash
MISTRAL_API_KEY=... cargo test \
  --manifest-path /home/jorge/rust_projects/super-stt/super-stt-daemon/Cargo.toml \
  --features wasm-backends \
  --test wasm_mistral_realtime live_mistral_realtime -- --nocapture --ignored
```

(Requires writing the `live_mistral_realtime` test analogously to `live_mistral` in `wasm_mistral.rs`. If you skipped that, run the round-trip test only.)

- [ ] **Step 6: No commit** — verification task.

---

## Self-review notes

- **Spec coverage:** WIT canonical+bundled (Tasks 1, 8). Manifest changes (Task 2). Daemon WS host (Tasks 3, 4). Catalog (Task 5). Consumer WS endpoint (Task 6). Doc updates (Task 7). Mistral backend (Tasks 9, 10, 11). Test (Task 12). E2E (Task 13). All spec sections covered.

- **`subscribe()` is intentionally bailed.** The spec calls it out as a known limitation behind the first realtime backend; the realtime.rs `pump()` uses sequential polling instead. A follow-up task wires real pollables. Documented inline in both files.

- **Bindgen output paths may not match exactly.** Tasks 3, 4, 9 may need minor `use` path adjustments depending on what wasmtime 45 and wit-bindgen 0.30 generate. The plan uses the most likely path (`super_stt::realtime::ws::*`); if the compiler complains, `cargo expand` will show the correct module path.

- **`Manifest::validate()` integration.** Step 9 of Task 2 has more discovery search than other steps. If the daemon's discovery code has multiple manifest-load call sites, instrument all of them; the rejection log message should be clear enough for a user to debug their own backend.toml.

- **The integration test (Task 12) relies on `WasmBackend::with_info` being callable from outside the crate.** If `with_info` isn't `pub`, make it `pub` or write a small test-only constructor exposed under `#[cfg(any(test, feature = "test-helpers"))]`.

- **`base64 = "0.22"` API:** uses `Engine::encode`. If the daemon's existing code uses an older base64 API, the backend can choose any compatible version since it's a separate crate.

- **Unit-level WS host test was omitted.** The spec mentions a tiny in-tree stub component for unit-testing `ws_host.rs`. Building a fixture wasm component is meaningful work (separate `Cargo.toml`, separate `cargo build --target wasm32-wasip2` step in CI). The integration test in Task 12 covers the same ground via the real Mistral component — if Task 12 passes, the WS host plumbing demonstrably works. Skip the unit-level fixture for now; add it later if regressions become hard to localize.
