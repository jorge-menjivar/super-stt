# `POST /transcribe`

> **Realtime models** — models with `realtime = true` in their backend
> configuration — are driven over
> [`GET /v1/transcribe/realtime`](../../backend/contract.md#consumer-facing-endpoint)
> (a WebSocket endpoint) rather than this route. `POST /v1/transcribe` is for
> non-realtime models only.

Start a transcription. The same endpoint covers four use cases,
dispatched on the request body:

| Use case                                  | `audio_data`    | `stream_realtime` | `wait`   | Response shape                                          |
|-------------------------------------------|-----------------|-------------------|----------|---------------------------------------------------------|
| Daemon-mic, fire-and-forget               | absent          | (ignored)         | `false`  | `202` with `{ message: "Recording started" }`           |
| Daemon-mic, wait for final result         | absent          | `false`           | `true`   | `200 text/event-stream` with a single `event: done`     |
| Daemon-mic, stream preview + final        | absent          | `true`            | `true`   | `200 text/event-stream` with `event: preview` frames then `event: done` |
| Pre-captured audio (one-shot)             | `[f32]` present | (must be `false` or absent) | (implicit `true`) | `200` JSON with `{ "transcription": "..." }`     |

To stop an in-flight daemon-mic capture, see
[`POST /transcribe/stop`](./transcribe/stop.md).

## Auth

- **Required scope:** `transcribe`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `transcribe` scope get `403 scope_denied`.

## `POST /transcribe`

**Request body:**

```jsonc
{
  // Optional. Present = pre-captured audio; daemon will not touch the mic.
  // Absent = daemon captures from its own microphone.
  "audio_data":  [0.012, -0.034, …],
  "sample_rate": 16000,

  // BCP-47 tag or "auto". When omitted, the daemon supplies the configured
  // language for the active model (see /v1/language and
  // /v1/active_model/language); a model that doesn't support the resolved
  // value falls back to its primary_language.
  "language":    "en",

  "data": {
    // Stream incremental `event: preview` SSE frames before the
    // final `event: done`. Default: false.
    "stream_realtime": true,

    // Mic-capture options (ignored when audio_data is present).
    // Default false. When true the final transcription is typed
    // into the focused window via the configured WriteMethod.
    "write_mode":      false,

    // Per-request override for the configured stop mode. One of:
    //   "silence" | "silence-and-manual" | "manual-only"
    "stop_mode":       "manual-only",

    // Default false. true = hold the response open until the
    // transcription is delivered.
    "wait":            true,

    // Per-request override for the preview_typing config flag.
    "preview":         true
  }
}
```

**Response shapes** (per use case):

| Path                                | Response                                                                |
|-------------------------------------|-------------------------------------------------------------------------|
| Daemon-mic, `wait: false`           | `202` with `{ "status": "success", "message": "Recording started" }`     |
| Daemon-mic, `wait: true`, no stream | `200 text/event-stream` with a single `event: done` carrying `{ "transcription": "..." }` |
| Daemon-mic, `wait: true`, streaming | `200 text/event-stream`: zero or more `event: preview` / `data: { "text": "..." }` blocks, then a single `event: done` / `data: { "transcription": "..." }` block |
| Pre-captured (`audio_data`)         | `200` with `{ "status": "success", "transcription": "..." }`             |

**SSE events emitted on a streaming response:**

| `event:`   | `data:` payload                       | When                                                                  |
|------------|---------------------------------------|-----------------------------------------------------------------------|
| `preview`  | `{ "text": "hello wor…" }`            | Streaming preview while audio keeps arriving                          |
| `done`     | `{ "transcription": "hello world" }`  | Final transcription; stream closes after this                         |
| `error`    | `{ "message": "..." }`                | Fatal error before `done`; stream closes after this                   |

The daemon also writes SSE comment frames (lines starting with `:`) —
an initial `: stream-open` right after the response headers and
periodic `: keepalive` comments every few seconds while no event is
flowing. Clients should ignore these per the SSE spec. They exist
because the daemon has long silent phases (stretches of a capture
that the model hasn't produced preview text for yet, plus the final
transcription pass after capture ends, which on CPU can run for
tens of seconds). Without the comment frames the underlying HTTP
connection would go idle and intermediaries (hyper's client, any
proxy in between) would drop it before the `done` event lands.

**Stopping early via socket disconnect:** for any `POST /transcribe`
issued with `wait: true`, closing the HTTP connection acts as an
implicit stop signal — the disconnect is detected on the next SSE
write, the capture ends, and any output not yet sent is dropped.
For `wait: false` (fire-and-forget) the connection is closed *by
design* immediately after `202 Accepted`; to stop a fire-and-forget
recording use [`POST /transcribe/stop`](./transcribe/stop.md).

**`POST /transcribe` never doubles as a stop signal.** Issuing it
while a daemon-mic capture is already running returns
`409 recording_in_progress`. To implement toggle behavior, clients
should consult `busy` on [`GET /status`](./status.md) and
route the request to [`POST /transcribe/stop`](./transcribe/stop.md)
when a capture is already in progress.

**Errors:**

| HTTP | `message`                          | Meaning                                                                |
|------|------------------------------------|------------------------------------------------------------------------|
| 400  | `stream_realtime_with_audio_data`  | Request carried both `audio_data` and `stream_realtime: true`           |
| 401  | `invalid_session`                  | Token unknown / expired / `exe_changed` — re-auth and retry             |
| 403  | `scope_denied`                     | Token lacks the `transcribe` scope                                      |
| 409  | `recording_in_progress`            | A daemon-mic capture was already running; check `busy` on `/status` and call `/transcribe/stop` instead |
| 429  | `rate_limited`                     | Per-client rate limit hit; back off and retry                           |
| 503  | `connection_rejected`              | Server refused the connection                                           |

Once an SSE response has started (`200 text/event-stream`), late
errors arrive as an in-stream `event: error` block followed by the
connection closing.
