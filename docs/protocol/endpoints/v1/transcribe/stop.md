# `POST /transcribe/stop`

Stop an in-flight daemon-mic capture. Idempotent: calling it when
nothing is running, or twice in quick succession, returns success
with an informational message rather than an error.

> **Most apps don't need this endpoint.** If your `/transcribe`
> connection is still open (`wait: true`), just close the HTTP
> connection — client disconnect is treated as an implicit stop.
> `/transcribe/stop` only exists for the two cases where
> socket-close isn't available:
>
> 1. You sent [`POST /transcribe`](../transcribe.md) with
>    `wait: false`; the connection is closed by design, so there's
>    nothing to disconnect.
> 2. You're stopping a recording started by a different process
>    (a panel applet stopping a hotkey daemon's recording, etc.).

`/transcribe/stop` only applies to the daemon-mic capture path. A
pre-captured (`audio_data`) request is one-shot and synchronous —
there is nothing to stop.

This endpoint **does not affect** the connection that issued the
matching `/transcribe`. If that caller used `wait: true`, it
continues to read `event: preview` blocks and the final `event:
done` block on its own connection — `/transcribe/stop` simply
causes the capture to end sooner. The final result is always
delivered on the connection that opened it, not on the connection
that issued the stop.

## Auth

- **Required scope:** `transcribe`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `transcribe` scope get `403 scope_denied`.

## `POST /transcribe/stop`

**Request:**

```http
POST /transcribe/stop HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

No request body.

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":  "success",
  "message": "Recording stop signal sent"
}
```

The `message` field carries one of:

| `message`                                       | Meaning                                                                         |
|-------------------------------------------------|---------------------------------------------------------------------------------|
| `"Recording stop signal sent"`                  | A daemon-mic capture was running and the active stop mode allows manual stop.   |
| `"No recording in progress"`                    | Nothing to stop right now.                                                      |
| `"Manual stop not enabled in current mode"`     | The active stop mode is `silence-only`; manual stop is disabled.                |
| `"Transcription in progress, please wait"`      | Capture has already ended; the model is decoding. The result will arrive on the connection that started it. |

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed` — re-auth and retry   |
| 403  | `scope_denied`    | Token lacks the `transcribe` scope                           |
