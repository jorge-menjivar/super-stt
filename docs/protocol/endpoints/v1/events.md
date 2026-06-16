# `GET /events`

Long-lived Server-Sent Events subscription. One connection delivers
every event in the requested topic set until either side closes
the stream. The byte-level SSE mechanics — comment keep-alives,
slow-consumer behavior, revoked / shutdown frames — live in
[`transport.md`](../../transport.md).

## Auth

- **Required scope:** any valid token, then **per topic**. Each requested topic
  is gated by the scope that grants it (see the Topics tables below):
  `recording_events`, `audio_visualization`, `global_transcriptions`, or
  `daemon_status`.
- `Authorization: Bearer <session_token>` is required.
- Requesting a topic the token's scopes don't grant fails the **whole**
  subscription with `403 scope_denied` before the stream opens — partial
  subscriptions are not supported.

## `GET /events`

**Request:**

```http
GET /events?topics=recording_state,frequency_bands HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Accept: text/event-stream
```

| Query param | Required | Notes                                                                            |
|-------------|----------|----------------------------------------------------------------------------------|
| `topics`    | yes      | Comma-separated list of topic names. Repeating `?topics=` is also accepted.      |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: text/event-stream
Cache-Control: no-store

event: subscribed
data: {"client_id":"sub_…","subscribed_to":["recording_state","frequency_bands"]}

event: recording_state
data: {"is_recording":true}

event: frequency_bands
data: {"bands_b64":"…","sample_rate":16000.0,"total_energy":0.0042}

…
```

The very first frame is always `event: subscribed`, carrying the
assigned subscriber id and the resolved topic list. Subsequent
frames have `event:` set to the topic name and `data:` set to the
topic-specific JSON payload (no outer wrapper).

The stream may end with `event: shutdown` (the daemon is going
away) or `event: revoked` (the session is no longer accepted —
reasons include `expired`, `exe_changed`, etc.). After a `revoked`
frame the client must re-issue
[`POST /auth/request`](./auth/request.md) before reopening.

## Topics

Each topic lists the scope a token must hold to subscribe to it. A token
requesting a topic outside its granted scopes gets `403 scope_denied` for the
whole subscription.

### Recording state

| Topic                  | Scope              | Payload                                                                                   |
|------------------------|--------------------|-------------------------------------------------------------------------------------------|
| `recording_started`    | `recording_events` | `{ client_id, timestamp, write_mode }`                                                    |
| `recording_stopped`    | `recording_events` | `{ client_id, timestamp }` — emitted when mic capture ends, before transcription           |
| `recording_state`      | `recording_events` | `{ is_recording: bool }` — `true` at capture start, `false` when the mic releases (mic-capture phase only; not the whole cycle) |
| `transcribing_started` | `recording_events` | `{ client_id, timestamp }` — model decode of the captured audio has begun                 |
| `transcribing_stopped` | `recording_events` | `{ client_id, timestamp, transcription_success, error? }` — decode + typing finished       |

For one daemon-mic capture the lifecycle events fire in this order:
`recording_started` → (`partial_stt` … — `global_transcriptions` scope only) → `recording_stopped` →
`transcribing_started` → `final_stt` (global_transcriptions scope only) → `transcribing_stopped`.
`recording_state{is_recording:true}` accompanies `recording_started` and
`recording_state{is_recording:false}` accompanies `recording_stopped` — i.e.
the visualization signal drops at mic-stop, independent of the transcription
that follows.

**Lifecycle terminal and optional events:**
- `transcribing_started` is emitted only when model decode actually begins
  (Phase 4). If the cycle fails during audio capture it is skipped entirely.
- `final_stt` is emitted only on a successful transcription (including "no
  speech detected", where `text` is empty). A failed cycle reports its error
  via `transcribing_stopped.error`; no `final_stt` is emitted.
- `transcribing_stopped` always closes the cycle — success or failure —
  and is the unconditional signal clients use to return to idle, regardless
  of whether they observed `transcribing_started` or `final_stt`.

### Audio fan-out

| Topic             | Scope                 | Payload                                                                                   |
|-------------------|-----------------------|------------------------------------------------------------------------------------------|
| `frequency_bands` | `audio_visualization` | `{ "bands_b64", "sample_rate", "total_energy" }` — base64-encoded f32 visualization bands |

Raw PCM is not exposed on the wire; the daemon computes the frequency bands and
broadcasts only those.

### Transcription

| Topic         | Scope                   | Payload                                              |
|---------------|-------------------------|------------------------------------------------------|
| `partial_stt` | `global_transcriptions` | `{ "text", "confidence" }` — live preview text       |
| `final_stt`   | `global_transcriptions` | `{ "text", "confidence" }` — final transcription     |

These carry the transcription text of every app, not just the subscriber's own —
hence the dedicated high-sensitivity scope. See
[global_transcriptions.md](../../scopes/global_transcriptions.md).

`confidence` is best-effort: backends that don't expose a confidence score
report `1.0`.

### Daemon status

| Topic                   | Scope           | Payload                                                                                                                                                                                                                                                                          |
|-------------------------|-----------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `daemon_status_changed` | `daemon_status` | Heterogeneous; the `status` field discriminates: `loading_model`, `ready` (carries `model_loaded` and optionally `actual_device` / `preferred_device` / `model_name`), `model_switched`, `switching_device`, `loading_model_for_device`, `device_switch_error`, `active_backend_changed` (carries `source` — the active backend's repo id, or `null` when the backend is cleared). Always includes `timestamp`. |
| `download_progress`     | `daemon_status` | `{ "model_name", "current_file", "file_index", "total_files", "bytes_downloaded", "total_bytes", "percentage", "status" ("downloading"/"loading_model"/"completed"/"cancelled"/"error"), "eta_seconds", "timestamp", "error"? }`. `bytes_downloaded`/`total_bytes`/`percentage` are per-file — all reset at each file boundary, so `percentage` (0–100) tracks the current file and the `file_index`/`total_files` counter conveys position in the set. `loading_model` is emitted once all files are on disk and the backend is loading weights into memory (an untracked phase); `percentage` pins to 100 for `loading_model` and `completed`. `error` is a human-readable failure detail present only on the terminal `status` = `"error"` tick (omitted otherwise), covering any switch failure — download, spawn, or weight-load — so a client can show why a switch failed without a second request. Throttled to ~1 % increments, plus an unthrottled publish on each file boundary and status change. |
| `registry_install`      | `daemon_status` | Backend-registry install / refresh progress — a serialized registry event (`install.progress` / `install.completed` / `install.failed` / `refresh.completed` / `refresh.failed`). |

## Closing the stream

There is no explicit `unsubscribe` request — closing the HTTP
connection ends the subscription. The stream may also close from
the server side with one of:

| Closing frame      | `data:` payload                                                                  | Client should                                                                                  |
|--------------------|----------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------|
| `event: shutdown`  | `{}`                                                                             | Reconnect later — the daemon is restarting / going away.                                       |
| `event: revoked`   | `{ "reason": "expired" \| "exe_changed" \| ... }`                                | Treat the cached token as gone. Call [`POST /auth/request`](./auth/request.md) before retrying. |

## Errors

| HTTP | `message`             | Meaning                                                                |
|------|-----------------------|------------------------------------------------------------------------|
| 400  | `invalid_topic`       | `topics` was empty, contained an unknown name, or couldn't be parsed.   |
| 401  | `invalid_session`     | Token unknown / expired / `exe_changed` — re-auth and reopen.           |
| 403  | `scope_denied`        | A topic outside the token's granted scopes was requested.               |
| 503  | `connection_rejected` | Server refused the connection (overloaded).                             |
