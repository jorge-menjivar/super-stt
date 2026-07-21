# Building on Super STT

Super STT is designed to be built on. The daemon exposes a documented
HTTP protocol over a local socket, and the models it runs are out-of-tree
backends that anyone can author and publish. This directory is the
protocol reference; this page is the entry point to it.

There are two ways to build on Super STT:

- **[Build a client](#build-a-client)** — any app, in any language, that
  wants transcriptions, event streams, or control over recording.
- **[Add your own model](#add-your-own-model)** — package a speech model
  as a backend the daemon can install and run.

---

## Build a client

A client talks to the daemon over an HTTP/1.1 + JSON API on a Unix domain
socket (`$XDG_RUNTIME_DIR/stt/super-stt-http.sock`). No Rust required —
`curl`, Python, Node, or anything with an HTTP client works.

```bash
# 1. Ask for consent. The user approves your app once, for the scopes you
#    request; the daemon returns a session token bound to your binary.
curl --unix-socket "$XDG_RUNTIME_DIR/stt/super-stt-http.sock" \
     -X POST http://stt.local/auth/request \
     -H 'Content-Type: application/json' \
     -d '{"app_name":"My App","scopes":["transcribe","status"],"version":"0.1"}'
# → { "session_token": "stt_…", "scopes": [...], "expires_at": "…" }

# 2. Send the token on every subsequent request.
curl --unix-socket "$XDG_RUNTIME_DIR/stt/super-stt-http.sock" \
     -X POST http://stt.local/transcribe \
     -H "Authorization: Bearer $STT_TOKEN" -d '{"wait":true}'
```

What the protocol gives you:

- **Consent-based auth.** A token is minted only after the user approves
  your app in a popup, and it is bound to your binary's identity — an app
  cannot widen its own permissions. See [auth.md](./auth.md).
- **Fine-grained scopes.** Request exactly what you need from `transcribe`,
  `status`, `settings`, `secrets`, `recording_events`,
  `audio_visualization`, `global_transcriptions`, `daemon_status`. Each is
  documented under [scopes/](./scopes/).
- **Live event streams.** Subscribe over Server-Sent Events
  (`GET /events?topics=…`) to recording state, audio frequency bands,
  model/download status, and final transcription text. See
  [endpoints/v1/events.md](./endpoints/v1/events.md).
- **Realtime transcription.** Realtime-capable models are driven over a
  WebSocket session at `/transcribe/realtime`.

Reference:

- [transport.md](./transport.md) — the wire shape: HTTP framing, SSE, error
  envelopes, connection lifecycle, and a minimal non-Rust client recipe.
- [auth.md](./auth.md) — the consent handshake, tokens, and the full scope
  catalog.
- [endpoints/](./endpoints/) — every endpoint, request/response by request.
- [scopes/](./scopes/) — what each scope unlocks.

## Add your own model

A model is delivered by a **backend**: an out-of-tree program the daemon
loads at runtime. Backends implement one HTTP-shaped contract, packaged in
one of two transports:

| Transport      | Use for                              | Isolation                                   |
|----------------|--------------------------------------|---------------------------------------------|
| WASM component | Cloud / API providers, light CPU     | wasmtime sandbox; network egress allowlisted |
| Native subprocess | Local, GPU-accelerated models     | Network-isolated; systemd + seccomp hardened |

To make a model available in Super STT:

1. **Build a backend** implementing the daemon↔backend contract —
   [backend/contract.md](./backend/contract.md), plus
   [backend/wasm.md](./backend/wasm.md) or
   [backend/subprocess.md](./backend/subprocess.md) for your transport, and
   [backend/config.md](./backend/config.md) for the `backend.toml` manifest.
2. **Publish it** by opening a PR that adds your repo to the registry — see
   [registry/README.md](../../registry/README.md). A nightly job discovers
   your releases and publishes them to the catalog every user browses;
   afterwards, shipping a new version is just tagging a release.

## Security model

Auth, transport, and the daemon's security posture are described in the
references above and in [../SECURITY.md](../SECURITY.md).
