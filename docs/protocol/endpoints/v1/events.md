# `GET /events`

Long-lived Server-Sent Events subscription. One connection delivers
every event in the requested topic set until either side closes
the stream. The byte-level SSE mechanics — comment keep-alives,
slow-consumer behavior, revoked / shutdown frames — live in
[`transport.md`](../../transport.md).

## Auth

- **Required scope:** `widget` (restricted topic set) or `settings`
  (full topic set).
- `Authorization: Bearer <session_token>` is required.
- `client` tokens get `403 scope_denied`.

A widget token requesting a settings-only topic fails the whole
subscription with `403 scope_denied` before the stream opens —
partial subscriptions are not supported.

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

Topics marked **W** are reachable by widget tokens; **S** marks
settings-only topics (widget tokens requesting them get `403
scope_denied`).

### Recording state

| Topic               | Scope | Payload                                                                              |
|---------------------|-------|--------------------------------------------------------------------------------------|
| `recording_started` | W     | `{ "client_id", "timestamp", "write_mode" }`                                          |
| `recording_stopped` | W     | `{ "client_id", "timestamp", "transcription_success", "error" }`                      |
| `recording_state`   | W     | `{ "is_recording": bool }` — coarse on/off ping                                       |

### Audio fan-out

| Topic             | Scope | Payload                                                                                                  |
|-------------------|-------|----------------------------------------------------------------------------------------------------------|
| `audio_samples`   | W     | `{ "sample_rate", "channels", "samples_b64" }` — base64-encoded f32 PCM (little-endian, 4 bytes/sample)  |
| `frequency_bands` | W     | `{ "bands_b64", "sample_rate", "total_energy" }` — base64-encoded f32 visualization bands                |

### Transcription preview

| Topic         | Scope | Payload                                              |
|---------------|-------|------------------------------------------------------|
| `partial_stt` | W     | `{ "text", "confidence" }` — live preview text       |
| `final_stt`   | W     | `{ "text", "confidence" }` — final transcription     |

### Daemon status (settings-only)

| Topic                   | Scope | Payload                                                                                                                                                                                                                                                                          |
|-------------------------|-------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `daemon_status_changed` | S     | Heterogeneous; the `status` field discriminates: `loading_model`, `ready` (carries `model_loaded` and optionally `actual_device` / `preferred_device` / `model_name`), `model_switched`, `switching_device`, `loading_model_for_device`, `device_switch_error`. Always includes `timestamp`. |
| `download_progress`     | S     | `{ "model_name", "current_file", "file_index", "total_files", "bytes_downloaded", "total_bytes", "percentage", "status" ("downloading"/"completed"/"cancelled"/"error"), "eta_seconds", "timestamp" }`. Throttled to ~1 % increments.                                              |

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
| 403  | `scope_denied`        | A topic outside the caller's scope was requested.                       |
| 503  | `connection_rejected` | Server refused the connection (overloaded).                             |
