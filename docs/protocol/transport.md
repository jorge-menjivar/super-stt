# Transport & Connection Lifecycle

This document describes how a client talks to the super-stt daemon at
the wire level: where the daemon listens, what HTTP shape every request
and response take, and how broadcast events are delivered over Server-
Sent Events.

It is the protocol-wide companion to:

- [auth.md](./auth.md) — authentication handshake and the full scope catalog
- the per-scope docs under [`scopes/`](./scopes/) — what each scope unlocks
- [`/events`](./endpoints/v1/events.md) — the SSE topic reference

The wire shape is HTTP/1.1, served on two transports that carry the
identical API: a Unix domain socket for native clients and a loopback
TCP listener for browsers. The endpoints, headers, JSON bodies and SSE
framing are the same on both — only who the daemon takes you to be
differs, which is what ["The TCP listener"](#the-tcp-listener) below is
about.

## Where the daemon listens

| Transport      | Address                                            | When                                     |
|----------------|----------------------------------------------------|------------------------------------------|
| Unix socket    | `$XDG_RUNTIME_DIR/stt/super-stt-http.sock`         | Always                                   |
| TCP            | `127.0.0.1:7300`                                   | On by default; `[http.tcp]` turns it off |

Native Linux clients should use the Unix socket. The daemon authenticates
peers there via `SO_PEERCRED` + `/proc/<pid>/exe`, which is what the
consent design depends on. The TCP listener is for browser clients, which
cannot dial a Unix socket and for which `SO_PEERCRED` does not exist — see
[auth.md](./auth.md#tcp-bound-clients).

A port that cannot be bound is logged and skipped rather than fatal: the
Unix socket is the daemon's primary transport, and refusing to start
because something else holds the port would take dictation, the applet and
the CLI down for the sake of an extra.

The socket path can be overridden via the `SUPER_STT_HTTP_SOCKET`
environment variable (tests use this to bind a unique socket per run).
The daemon and every in-tree client resolve the path through the same
helper, so the override applies to both ends — set it in a shared
environment and the daemon listener and its clients stay in agreement.

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
  "wait":        true,
  "stream_realtime": true,
  "stop_mode":   "manual_only"
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

Anything an HTTP/1.1 client library does — connection reuse,
pipelining, chunked transfer encoding — is supported. Whether you
open one connection per request or hold a keep-alive open is up to
you; authentication is per-request via the `Authorization` header.

The `Host` header is required by HTTP/1.1 but its value is ignored.
Use `stt.local`, `localhost`, or anything else.

## Authentication header

Every request after the initial `POST /auth/request` carries:

```http
Authorization: Bearer <session_token>
```

The session token is a 32-byte random value, hex-encoded, returned by
[`/auth/request`](./auth.md). It's validated on every request. On
failure you'll see:

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
denial), 5xx for server-side failures.

| HTTP status | When                                                        |
|-------------|-------------------------------------------------------------|
| 200 OK      | Successful read or mutation                                 |
| 202 Accepted| Successful mutation that runs asynchronously (e.g. `POST /pipeline/1/model`) |
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
GET /events?topics=recording_state,frequency_bands,daemon_status_changed,download_progress HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…
Accept: text/event-stream
```

```http
HTTP/1.1 200 OK
Content-Type: text/event-stream
Cache-Control: no-store

event: subscribed
data: {"client_id":"sub_…","subscribed_to":["recording_state","frequency_bands","daemon_status_changed","download_progress"]}

event: recording_state
data: {"is_recording":true}

event: daemon_status_changed
data: {"status":"loading_model","new_model":"whisper-base","timestamp":"2026-05-22T12:34:56Z"}

event: download_progress
data: {"model_name":"whisper-base","current_file":"model.safetensors","file_index":1,"total_files":3,"bytes_downloaded":12345,"total_bytes":45678,"percentage":27.0,"status":"downloading","eta_seconds":14,"timestamp":"2026-05-22T12:34:57Z"}

event: daemon_status_changed
data: {"status":"ready","model_loaded":true,"model_name":"whisper-base","timestamp":"2026-05-22T12:35:14Z"}
```

Conventions:

- The first frame is always `event: subscribed` with the assigned
  subscriber `client_id` and the resolved topic list. Topics the
  client requested but isn't allowed to see (e.g. a token without
  `daemon_status` asking for `daemon_status_changed`) cause a
  `403 scope_denied` before the stream opens; partial subscriptions
  aren't supported.
- Each subsequent frame's `event:` field is the topic name. The
  `data:` field is one JSON line carrying the topic-specific payload
  directly (no wrapper envelope). For example, `recording_state` is
  `{"is_recording":true}`; `daemon_status_changed` is
  `{"status":"…", …, "timestamp":"…"}`. See
  [`/events`](./endpoints/v1/events.md) for per-topic shapes and the
  scope each topic requires.
- Multi-line `data:` is permitted by SSE; events arrive as a single
  line per event.
- An SSE comment (line starting with `:`) arrives every 30 seconds
  as a keep-alive. Clients should ignore them; their job is to stop
  HTTP intermediaries from timing the connection out and to give
  clients a stable cadence to detect a wedged stream (if no comment
  or event arrives for a minute, the connection is dead — close it
  and reconnect).

The `topics` query parameter is comma-separated. Repeating it
(`?topics=a&topics=b`) is also accepted and merges to the same set.

### Audio fan-out is on the same stream

The audio fan-out — recording state, frequency bands, partial /
final STT — is just additional topics on the same SSE stream. There
is no separate UDP socket. Raw PCM is not exposed; the daemon
computes the frequency bands and broadcasts only those. See
[`/events`](./endpoints/v1/events.md) for the audio-specific topic
payloads.

For binary efficiency, frequency-band payloads use base64 inside the
JSON `data` field (`bands_b64`). The encoding overhead (~33 %) is
negligible on a local socket.

### Slow consumers

A subscriber that doesn't drain fast enough has its oldest queued
events dropped — the connection itself isn't closed, and the next
live event arrives normally. Clients that want to verify they
didn't miss critical state can issue a fresh `GET` on the relevant
resource (e.g. `GET /pipeline/1`) — those snapshots are
authoritative.

### Closing the stream

The client closes the stream by closing the underlying HTTP
connection. There is no `unsubscribe` request — the connection *is*
the subscription. The stream may also end on the server side, in
which case the last frame the client receives identifies why:

- `event: shutdown` / `data: {}` — the daemon is going away.
- `event: revoked` / `data: { "reason": "..." }` — the session is no
  longer accepted. Reasons include `expired`, `exe_changed` (the
  client's binary identity changed, see
  [auth.md](./auth.md#anti-replacement)), and any other
  revocation cause. The client must re-issue `/auth/request` before
  reopening the stream.

## Error responses

Every error returns a JSON body with the same shape:

```jsonc
{
  "status":     "error",
  "error_code": "recording_in_progress",
  "message":    "Cannot change the backend during active recording.",
  "data":       { "reason": "<machine-readable reason>", ... }
}
```

`error_code` is a stable, machine-readable `snake_case` identifier — the
field clients should switch on. It is present on every error the daemon
originates. `message` is a human-readable, single-line explanation suitable
for display; it is sanitized so secrets can't leak into the response body,
and its exact wording is **not** part of the contract (do not match on it).
The optional `data` carries additional structured detail.

The HTTP status is derived from `error_code` (e.g. a state-conflict code →
`409`, a bad-input code → `400`); an error the daemon cannot classify carries
no `error_code` and surfaces as `500`.

> **Compatibility.** `error_code` was introduced after `message`. Earlier
> clients switched on `message`, and the auth identifiers (`invalid_session`,
> `scope_denied`) still appear there verbatim for that reason. New clients
> should switch on `error_code`.

## The TCP listener

The same HTTP API is served on `127.0.0.1:7300`. The endpoints, the
`Authorization` header and the JSON bodies are identical; three things
around them are not.

1. **Auth identity.** `SO_PEERCRED` does not apply, so a caller here is
   identified by its `Origin` header — a claim from the browser rather
   than a fact from the kernel. The consent dialog names the origin, and
   says in so many words that this is the weaker of the two checks. A
   token is bound to the whole identity, so one minted for an origin is
   refused to any other origin and to every binary, and the reverse.
   `app_name` is not part of the identity: it is self-reported, and
   renaming your app neither widens nor narrows what a token covers.

2. **An origin allowlist.** `[http.tcp].allowed_origins` in
   `daemon.toml` says which origins may call at all. The default is
   `["*"]` — any page may *ask*, and the consent dialog is what it has
   to get past. An origin that is not on the list is refused with `403`
   and `data.reason = "origin_not_allowed"`, before authentication,
   before consent, and before any dialog can reach the user's screen.
   An empty list admits nothing, which is the deliberate lockdown.

   The daemon enforces this itself rather than relying on the CORS
   headers it also sends. CORS instructs a *browser* to refuse a
   response; a non-browser client on the same port ignores it entirely,
   so it cannot be what guards the surface.

3. **CORS and Private Network Access headers**, on TCP responses only —
   the Unix socket never sees them, since nothing that reaches it is
   subject to them. `Access-Control-Allow-Origin` echoes the validated
   origin rather than `*`, so the header can never name an origin the
   user did not authorize. A preflight is answered directly, and a
   preflight carrying `Access-Control-Request-Private-Network: true`
   (Chrome's check for a public page reaching a loopback address) is
   answered with `Access-Control-Allow-Private-Network: true`.

There is no TLS. A loopback address is a [potentially trustworthy
origin](https://w3c.github.io/webappsec-secure-contexts/#is-origin-trustworthy),
so `https://page` → `http://127.0.0.1:7300` is exempt from mixed-content
blocking and needs no certificate.

Native clients on the Unix socket get peer-credential-verified identity.
Browser clients on TCP get origin-verified identity. Both hit the same
endpoints.

### Port 7300

`7300`–`7309` is the block reserved for the Super family, one port per
daemon: `7300` Super STT, `7301` Super TTS, the rest unused for now.

The port is fixed rather than OS-chosen because it is half of a browser
client's origin: one that moved on every restart would invalidate the
other side's bookmarks and its stored token binding. Change it with
`[http.tcp].port` if something else on your machine wants it.

## Authoring a non-Rust client

The minimal recipe for a fresh client of any scope:

1. Use any HTTP client your language has. Examples:
   ```bash
   curl --unix-socket "$XDG_RUNTIME_DIR/stt/super-stt-http.sock" \
        -X POST http://stt.local/auth/request \
        -H 'Content-Type: application/json' \
        -d '{"app_name":"My App","scopes":["transcribe","status"],"version":"0.1"}'
   ```
   ```python
   import requests_unixsocket
   s = requests_unixsocket.Session()
   r = s.post("http+unix://%2Frun%2Fuser%2F1000%2Fstt%2Fsuper-stt-http.sock/auth/request",
              json={"app_name": "My App", "scopes": ["transcribe", "status"], "version": "0.1"})
   token = r.json()["session_token"]
   ```
   ```javascript
   // Node
   const http = require('http');
   const req = http.request({
     socketPath: '/run/user/1000/stt/super-stt-http.sock',
     method: 'POST',
     path: '/auth/request',
     headers: {'Content-Type': 'application/json'}
   }, ...);
   ```

2. Persist the returned `session_token` in your platform's keyring.

3. For commands, send `Authorization: Bearer <token>` on every request:
   ```bash
   curl --unix-socket "$XDG_RUNTIME_DIR/stt/super-stt-http.sock" \
        -X POST http://stt.local/transcribe \
        -H "Authorization: Bearer $STT_TOKEN" \
        -H 'Content-Type: application/json' \
        -d '{"data":{"wait":true,"stream_realtime":true}}'
   ```

4. For event streams, use any HTTP client that supports SSE (or just
   read line-by-line):
   ```bash
   curl --unix-socket "$XDG_RUNTIME_DIR/stt/super-stt-http.sock" \
        -N \
        "http://stt.local/events?topics=recording_state,daemon_status_changed,download_progress" \
        -H "Authorization: Bearer $STT_TOKEN"
   ```

5. On any 401 with `message: "invalid_session"`, run the auth flow
   again and retry the original request.
