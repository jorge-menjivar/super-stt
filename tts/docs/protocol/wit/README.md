# Super TTS custom WIT packages

This directory holds custom WIT package definitions that are part of the Super TTS backend protocol but not yet standardized in WASI.

## `realtime.wit` — `super-engine:realtime@0.1.0`

Defines two interfaces a wasm backend uses for realtime (WebSocket-based) sessions:

- `ws` — outgoing WebSocket client. The backend imports this to reach an upstream realtime API. The daemon enforces the backend's `[network].allowed_hosts` and SSRF resolver.
- `ws-server` — incoming WebSocket server. The backend exports this so the daemon can hand it a consumer WebSocket session.

A backend that needs realtime support:
- Declares `[capabilities] websocket = true` in `backend.toml`.
- Declares at least one `[[models]] realtime = true`.
- Imports `super-engine:realtime/ws` and exports `super-engine:realtime/ws-server` in its `realtime-backend` world.

## Cross-language consumption

The WIT is language-agnostic. Non-Rust backends generate their own bindings:

- Rust: `wit_bindgen::generate!({ path: "wit/realtime.wit", world: "realtime-backend" });`
- JavaScript/TS: `jco transpile` or `componentize-js`
- Python: `componentize-py -d wit/realtime.wit -w realtime-backend bindings src/bindings.py`
- Go (TinyGo): `wit-bindgen-go generate`

In-tree first-party backends vendor a byte-identical copy of this WIT into `backends/<name>/wit/realtime.wit`; the `just check-wit-sync` recipe enforces parity. Third-party / out-of-tree backends should pin to a specific revision.
