# WASM Backends

A WASM backend is a WebAssembly component the daemon loads and invokes
in-process. Use this transport for **cloud / API models** — and for
lightweight CPU work — where the backend is essentially an HTTP client that
shapes requests for a hosted provider. The WASM sandbox is a strong fit here:
the component holds no ambient authority, so the daemon can confine its
network egress precisely, at no latency cost for a network-bound backend.

This document is part of the [backend protocol](./contract.md). The contract
— the `/v1` routes, payloads, and lifecycle — is defined there; this
document covers only what is specific to the WASM transport. Configuration fields
are described in [config.md](./config.md).

A WASM backend declares `kind = "wasm"` and an `entrypoint` pointing at its
`.wasm` component.

## Transport

The backend is compiled to a `wasm32-wasip2` component that **exports
`wasi:http/incoming-handler`**. The daemon invokes that export in-process,
handing it the request — there is no socket and no separate process. The
component implements the [`/v1` routes](./contract.md#the-v1-contract) by
dispatching on the request path and method, returning the same payloads a
subprocess backend would serve, including the SSE form of
`POST /v1/transcribe`.

Because invocation is a direct in-process call, the WASM transport has no IPC
overhead and no socket lifecycle. "Spawning" a backend is instantiating the
component; tearing it down is dropping the instance.

## Network egress

A component has no ambient authority: it cannot open a socket on its own. Its
only path to the network is the **`wasi:http/outgoing-handler`** import,
which the daemon implements. The daemon enforces the configuration's
[`allowed_hosts`](./config.md#network) on every outbound request:

- The request authority (host and port) is checked against `allowed_hosts`
  before the request is dispatched. A request to any other host is rejected;
  the backend never reaches it.
- The daemon does **not** provide `wasi:sockets`, so raw TCP/UDP is
  impossible — HTTP through the allowlisted handler is the only egress.
- After resolving an allowed host, the daemon rejects requests that resolve
  to loopback, link-local, or private ranges — including the metadata
  address `169.254.169.254` — to prevent server-side request forgery.

A backend with an empty `allowed_hosts` has no network at all.

## Secrets and options

The active model (`x-stt-model`) and the secrets and options a backend
declares in its [configuration](./config.md) arrive as request headers on
every `/v1` request (see [request headers](./contract.md#request-headers)).
The component reads them from the incoming request's headers; it never sees
the keyring, and it needs no `wasi:config` import.

For an OpenAI backend declaring `OPENAI_API_KEY`, each `/v1/transcribe`
arrives with `x-stt-model` and `x-stt-secret-OPENAI_API_KEY`; the component
reads the key and sets `Authorization: Bearer <value>` on its outbound
request to
`api.openai.com`. It must not forward the injected header upstream.

## Model files

Cloud WASM backends declare no model files. If a WASM backend does declare
[`[[models.files]]`](./config.md#modelsfiles), the daemon provisions them
and grants the component read-only access to the backend directory as a
preopened filesystem; otherwise the component is given no filesystem access.

## Packaging

`entrypoint` names the component file relative to the backend directory
(e.g. `openai.wasm`). The daemon loads and instantiates it with wasmtime,
wiring only the imports the contract needs: the filtered
`wasi:http/outgoing-handler` and — only when the configuration declares
files — read-only access to the backend directory. Secrets and options
arrive as request headers, not as imports.

## Realtime (WebSocket)

A wasm backend that proxies an upstream realtime API (for example, a
streaming WebSocket transcription service) opts into a second interface pair
beyond `wasi:http`. The interface definitions are in
`docs/protocol/wit/realtime.wit`; the canonical package name is
`super-stt:realtime@0.1.0`.

### Opt-in

Declare `[capabilities] websocket = true` in `backend.toml` and mark at
least one model with `realtime = true`. A backend that declares
`websocket = true` without `realtime = true` on any model passes validation
but the capability is never invoked. A `subprocess` backend may not declare
`websocket = true`; it is rejected at discovery.

### WIT interfaces

The `realtime-backend` world the component must implement:

```wit
world realtime-backend {
    import wasi:http/outgoing-handler@0.2.0;
    import wasi:io/poll@0.2.0;
    import ws;            // super-stt:realtime/ws
    export wasi:http/incoming-handler@0.2.0;
    export ws-server;     // super-stt:realtime/ws-server
}
```

**Imported: `super-stt:realtime/ws`**

Provides `connect(url, headers) -> ws-stream` for opening an outgoing
WebSocket to an upstream service. Returns the host-owned `ws-stream`
resource, which exposes `send-text`, `send-binary`, `recv`, `subscribe`,
and `close`. The same allowlist and SSRF enforcement applied to
`wasi:http/outgoing-handler` applies to `connect`: the URL's host must
appear in `[network].allowed_hosts`, the URL scheme must be `ws://` or
`wss://`, and hosts that resolve to loopback, link-local, or private ranges
are rejected.

The `consumer-stream` resource (host-owned, handed in by `ws-server.handle`)
provides the same five methods for communicating with the consumer.

**Exported: `super-stt:realtime/ws-server`**

```wit
handle: func(
    headers: list<tuple<string, list<u8>>>,
    consumer: consumer-stream,
) -> result<_, ws-error>;
```

The daemon invokes `handle` once per consumer realtime session. `headers`
carries the daemon-injected `x-stt-*` context (model name, secrets, options)
as UTF-8 key/value pairs. `consumer` is the host-owned consumer WebSocket.
The component pumps frames between `consumer` and any upstream connection
it opens, returning when the session ends.

### Canonical WIT location

The canonical WIT is at `docs/protocol/wit/realtime.wit`. In-tree backends
vendor a byte-identical copy into `backends/<name>/wit/realtime.wit`; the
`just check-wit-sync` recipe enforces parity. Out-of-tree backends should
pin to a specific revision.

Language bindings:

| Language | Toolchain |
|----------|-----------|
| Rust | `wit_bindgen::generate!({ path: "wit/realtime.wit", world: "realtime-backend" })` |
| JS/TS | `jco transpile` or `componentize-js` |
| Python | `componentize-py -d wit/realtime.wit -w realtime-backend` |
| Go (TinyGo) | `wit-bindgen-go generate` |

### Known limitation: `subscribe` not implemented

The `subscribe` method on both `ws-stream` and `consumer-stream` returns a
`wasi:io/poll` pollable, but the host-side implementation is not yet
complete. A guest must poll `recv` sequentially rather than multiplexing
via `wasi:io/poll`. Do not rely on `subscribe` to detect readability without
also calling `recv`.

## Implementation checklist

- Compile to a `wasm32-wasip2` component that exports
  `wasi:http/incoming-handler`.
- Declare `kind = "wasm"`, an `entrypoint`, the `allowed_hosts` the backend
  needs, and any `[[secrets]]` or `[[options]]` in [backend.toml](./config.md).
- Implement the [`/v1` routes](./contract.md#the-v1-contract) by dispatching
  on method and path; stream `event: preview` / `event: done` for
  `POST /v1/transcribe` when `options.stream_realtime` is set.
- Make all outbound calls through `wasi:http/outgoing-handler`; do not rely
  on raw sockets.
- Read secrets and options from the injected `x-stt-secret-*` and
  `x-stt-option-*` request headers; use a secret only to authenticate
  outbound calls, and never forward it upstream.
- For a cloud backend, report `state: "ready"` from `GET /v1/status` as soon
  as the component is instantiated — there are no weights to load.
- For a realtime backend: declare `[capabilities] websocket = true` and
  `realtime = true` on each realtime model; implement the `realtime-backend`
  world (import `super-stt:realtime/ws`, export `super-stt:realtime/ws-server`);
  poll `recv` sequentially rather than relying on `subscribe`.
