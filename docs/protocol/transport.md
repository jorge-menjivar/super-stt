# Transport & Connection Lifecycle

This document describes how a client talks to the super-stt daemon at
the wire level: where the daemon listens, what HTTP shape every request
and response take, and how broadcast events are delivered over Server-
Sent Events.

It is the protocol-wide companion to:

- [auth.md](./auth.md) — authentication handshake
- [client.md](./client.md) — client-scope endpoints
- [settings.md](./settings.md) — settings-scope endpoints
- [widget.md](./widget.md) — widget-scope endpoints

The wire shape is HTTP/1.1 over a Unix domain socket. A future config
flag will let the daemon bind a TCP listener with the same HTTP API
(see "Forward compatibility with TCP" below); the endpoints,
headers, JSON bodies, and SSE framing all stay the same.

## Where the daemon listens

| Transport      | Address                                       | When                                |
|----------------|-----------------------------------------------|-------------------------------------|
| Unix socket    | `$XDG_RUNTIME_DIR/stt/super-stt.sock`         | Always (default)                    |
| TCP            | `127.0.0.1:<configurable port>`               | Optional, opt-in via daemon config  |

Native Linux clients should use the Unix socket. The daemon authenticates
peers there via `SO_PEERCRED` + `/proc/<pid>/exe`, which is what the
consent design depends on. The TCP bind is intended for browser apps
where `SO_PEERCRED` isn't available — see
[auth.md](./auth.md#tcp-bound-clients).

The daemon serves the same routes on both transports.

## Wire shape: HTTP/1.1 + JSON

Every interaction except event streaming is one HTTP request and one
HTTP response. Standard HTTP semantics:

```http
POST /transcribe HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json
Content-Length: 178

{
  "audio_data":  null,
  "sample_rate": null,
  "language":    "en",
  "data":        { "wait": true, "stream_realtime": true,
                   "stop_mode": "manual-only" }
}
```

```http
HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: 60

{
  "status": "success",
  "transcription": "hello world"
}
```

Anything an HTTP/1.1 client library does — connection reuse, pipelining,
chunked transfer encoding — is fine. The daemon doesn't care whether
clients open one connection per request or hold a keep-alive open;
authentication is per-request via the `Authorization` header.

The `Host` header is required by HTTP/1.1 but the daemon ignores its
value. Clients can use `stt.local`, `localhost`, or anything else.

## Authentication header

Every request after the initial `POST /auth/request` carries:

```http
Authorization: Bearer <session_token>
```

The session token is a 32-byte random value, hex-encoded, returned by
[`/auth/request`](./auth.md). The daemon validates it on every request
by looking it up in its keyring, comparing the stored exe-path against
`/proc/<peer_pid>/exe`, and checking the expiry. On failure the daemon
responds:

```http
HTTP/1.1 401 Unauthorized
Content-Type: application/json

{
  "status": "error",
  "message": "invalid_session",
  "data": { "reason": "expired" }
}
```

The reason is one of `unknown`, `expired`, or `exe_changed`. The client
must re-issue `POST /auth/request` to obtain a fresh token.

## Request and response shapes

| Endpoint kind | HTTP                          | Body                              |
|---------------|-------------------------------|-----------------------------------|
| Read          | `GET /name`                   | none                              |
| Mutate        | `POST /name`                  | JSON arguments (or empty)         |
| Action        | `POST /name`                  | JSON arguments                    |
| Event stream  | `GET /events?topics=...`      | none, returns SSE                 |

Responses are always JSON with a top-level `status` field that is
`"success"` or `"error"`. The HTTP status code mirrors the JSON status:
2xx for success, 4xx for client errors (auth, validation, scope
denial), 5xx for daemon-internal failures.

| HTTP status | When                                                        |
|-------------|-------------------------------------------------------------|
| 200 OK      | Successful read or mutation                                 |
| 202 Accepted| Successful mutation that runs asynchronously (e.g. `POST /active_model`) |
| 400 Bad Request | Request validation failed (missing fields, bad enum)    |
| 401 Unauthorized | Missing or invalid `Authorization` header              |
| 403 Forbidden | Token valid but scope insufficient (`scope_denied`)       |
| 404 Not Found | Unknown endpoint                                          |
| 409 Conflict | Mutation rejected because of state (e.g. switch already in flight) |
| 429 Too Many Requests | Rate-limit hit                                    |
| 500 Internal Server Error | Daemon-side bug; details in logs              |

## Event streams: Server-Sent Events

Subscriptions use `GET /events?topics=...`. The daemon keeps the
connection open and writes one SSE frame per published event until the
client disconnects or the daemon shuts down.

```http
GET /events?topics=config_changed,model_switch_started,model_switch_progress,model_switch_completed,model_switch_failed HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…
Accept: text/event-stream
```

```http
HTTP/1.1 200 OK
Content-Type: text/event-stream
Cache-Control: no-store

event: subscribed
data: {"client_id":"sub_…","subscribed_to":["config_changed", "..."]}

event: config_changed
data: {"client_id":"src_…","timestamp":"2026-05-03T12:34:56Z","data":{"key":"volume","value":75}}

event: model_switch_progress
data: {"client_id":"src_…","timestamp":"2026-05-03T12:34:57Z","data":{"phase":"downloading","target":{"model":"whisper-base","provider":"local_whisper","source":"builtin"},"download":{"current_file":"model.safetensors","file_index":1,"total_files":3,"bytes_downloaded":12345,"total_bytes":45678,"percentage":27.0,"eta_seconds":14}}}
```

Conventions:

- The first frame is always `event: subscribed` with the assigned
  `client_id` (the subscriber's id, distinct from the originator field
  on subsequent events) and the resolved topic list.
- Each subsequent frame's `event:` field is the topic name. The `data:`
  field is one JSON line carrying `client_id` (originator), `timestamp`,
  and a topic-specific `data` payload.
- Multi-line `data:` is permitted by SSE but the daemon always emits a
  single line per event.
- The daemon writes SSE comments (lines starting with `:`) every 30
  seconds as keep-alive ping. Clients should ignore them; they exist
  so HTTP intermediaries don't time the connection out.

The `topics` query parameter is comma-separated. Repeating it
(`?topics=a&topics=b`) is also accepted and merges to the same set.

### Audio fan-out is on the same stream

The widget scope's audio fan-out — recording state, raw PCM samples,
frequency bands, partial/final STT — is just additional topics on
the SSE stream. There is no separate UDP socket. See
[widget.md](./widget.md) for the audio-specific topic payloads.

For binary efficiency, audio sample payloads use base64 inside the
JSON `data` field. At ~30 KB/s the encoding overhead (~33%) is
negligible on a local socket.

### Slow consumers

Each subscriber gets a bounded outgoing queue. A subscriber that
doesn't drain fast enough has its oldest events dropped, not the
connection itself dropped. The next live event arrives normally. This
matches the loss-on-overload behavior the audio fan-out had over UDP,
without needing a second transport. Lag is logged on the daemon side.

A subscriber that wants to verify it didn't miss critical events can
issue a fresh `GET` on the relevant resource (e.g.
`GET /active_model`) — the snapshot reflects current state
authoritatively.

### Closing the stream

The client closes the stream by closing the underlying HTTP
connection. There is no `unsubscribe` request — the connection is the
subscription. The daemon closes the stream on:

- Daemon shutdown (writes the SSE frame `event: shutdown\ndata: {}\n\n` if
  there's time, then closes).
- Token revocation or expiry detected during a periodic re-validation
  pass (writes `event: revoked\ndata: {"reason":"..."}\n\n`, then closes).
- `/proc/<peer_pid>/exe` no longer matches the keyring entry (same
  `revoked` event, reason `exe_changed`).

## Error responses

Every error returns a JSON body with the same shape:

```jsonc
{
  "status":  "error",
  "message": "<short identifier>",
  "data":    { "reason": "<machine-readable reason>", ... }
}
```

`message` is a stable identifier suitable for clients to switch on.
The optional `data` carries machine-readable detail. Free-form text
goes in the daemon's logs, not the wire response — error messages on
the wire are sanitized to one line so secrets in formatted error
chains don't leak.

Set `SUPER_STT_DEBUG_ERRORS=1` in the daemon environment to disable
sanitization while developing.

## Forward compatibility with TCP

When the daemon's TCP listener is enabled, the same HTTP API is served
on `127.0.0.1:<port>`. Three things change:

1. **Auth identity.** `SO_PEERCRED` doesn't apply, so the consent
   popup prompts for a *web origin* (the `Origin` request header)
   rather than an exe path. Tokens issued under TCP are keyed by
   `(app_name, web_origin)` instead of `(app_name, exe_path)`. The
   wire shape (Authorization header, JSON bodies) is identical.

2. **CORS / Private Network Access headers** are emitted on the TCP
   responses. Browsers enforce them; raw TCP clients ignore them. The
   daemon emits them on the Unix socket too, where they're harmlessly
   ignored.

3. **TLS.** Chrome's Private Network Access spec wants HTTPS for
   public-site → localhost requests. The daemon presents a self-signed
   cert for `localhost` when TCP-bound; the user accepts it once.

Native Linux clients on the Unix socket get peer-credential-verified
identity. Browser clients on TCP get origin-verified identity. Both
hit the same endpoints. A Rust shared client can target either with
just a different `Connector`.

## Authoring a non-Rust client

The minimal recipe for a fresh client of any scope:

1. Use any HTTP client your language has. Examples:
   ```bash
   curl --unix-socket "$XDG_RUNTIME_DIR/stt/super-stt.sock" \
        -X POST http://stt.local/auth/request \
        -H 'Content-Type: application/json' \
        -d '{"app_name":"My App","scope":"client","version":"0.1"}'
   ```
   ```python
   import requests_unixsocket
   s = requests_unixsocket.Session()
   r = s.post("http+unix://%2Frun%2Fuser%2F1000%2Fstt%2Fsuper-stt.sock/auth/request",
              json={"app_name": "My App", "scope": "client", "version": "0.1"})
   token = r.json()["session_token"]
   ```
   ```javascript
   // Node
   const http = require('http');
   const req = http.request({
     socketPath: '/run/user/1000/stt/super-stt.sock',
     method: 'POST',
     path: '/auth/request',
     headers: {'Content-Type': 'application/json'}
   }, ...);
   ```

2. Persist the returned `session_token` in your platform's keyring.

3. For commands, send `Authorization: Bearer <token>` on every request:
   ```bash
   curl --unix-socket "$XDG_RUNTIME_DIR/stt/super-stt.sock" \
        -X POST http://stt.local/transcribe \
        -H "Authorization: Bearer $STT_TOKEN" \
        -H 'Content-Type: application/json' \
        -d '{"data":{"wait":true,"stream_realtime":true}}'
   ```

4. For event streams, use any HTTP client that supports SSE (or just
   read line-by-line):
   ```bash
   curl --unix-socket "$XDG_RUNTIME_DIR/stt/super-stt.sock" \
        -N \
        "http://stt.local/events?topics=config_changed,model_switch_progress" \
        -H "Authorization: Bearer $STT_TOKEN"
   ```

5. On any 401 with `message: "invalid_session"`, run the auth flow
   again and retry the original request.

The Rust shared client (`super_stt_shared::daemon::client`) wraps all
of this into typed function calls.
