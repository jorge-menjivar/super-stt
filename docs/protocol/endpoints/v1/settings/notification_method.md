# `/settings/notification_method`

Read and set how the daemon surfaces a recording failure to the user.

A failure — no model loaded, the recorder could not start, capture died
partway, or the model could not transcribe the audio — is reported to the
caller regardless of this setting: as the direct error response (e.g. `409
model_not_loaded`) or, once an SSE stream has started, via the `error` event.
This setting controls the additional, human-facing notice.

| Method  | Notes                                                                                          |
|---------|------------------------------------------------------------------------------------------------|
| `auto`  | Send a desktop notification; if it cannot be delivered, type the notice instead (the default). |
| `dbus`  | Send a desktop notification only. If it cannot be delivered, the failure is logged and nothing is shown. |
| `typed` | Type a fixed notice into the focused window.                                                   |
| `off`   | Log the failure only; never surface it.                                                        |

Desktop notifications use the freedesktop Desktop Notifications interface
(`org.freedesktop.Notifications`) on the session bus, so they work on any
desktop that provides a notification server.

Typing requires a `POST /transcribe` with `write_mode: true`. For a recording
that is not in write mode, `typed` — and `auto` once notification delivery has
failed — logs the failure instead.

The new method takes effect on the next recording.

## What the user sees

The two channels carry different amounts of detail, because they land in
different places.

A notification has a summary and a body of its own, and its app name and icon
are supplied separately — so the summary names the failure and the body gives
the reason:

| Failure                       | Summary                     | Body                             |
|-------------------------------|-----------------------------|----------------------------------|
| No model is loaded            | `No model loaded`           | `Load a model and try again.`    |
| The recorder could not start  | `Could not start recording` | The reason, from the daemon      |
| Capture died partway          | `Recording failed`          | The reason, from the daemon      |
| The audio was not transcribed | `Transcription failed`      | The reason, from the backend     |

Reasons authored by a backend are prefixed `Backend error:`, so a failure the
daemon is only relaying is never mistaken for one of its own. Backend text is
untrusted: it is flattened to a single line, escaped so a notification server
that renders markup in the body cannot be driven from it, and clamped to 300
characters. A failure that arrives with no reason to report falls back to a
fixed sentence rather than an empty body.

Typing has nowhere to put a reason: the notice lands in whatever window the user
has focused, in among their own text. It is one fixed, daemon-authored string
per failure, bracketed so it cannot be read as transcript, and it never carries
backend text:

```
[Super STT: no model loaded]
[Super STT: could not start recording]
[Super STT: recording failed]
[Super STT: transcription failed]
```

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /settings/notification_method`

**Request:**

```http
POST /settings/notification_method HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "method": "dbus"
}
```

| Field    | Type   | Required | Notes                                          |
|----------|--------|----------|--------------------------------------------------|
| `method` | string | yes      | One of `auto`, `dbus`, `typed`, `off`          |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":              "success",
  "notification_method": "dbus"
}
```

**Errors:**

| HTTP | `message`                     | Meaning                                        |
|------|-------------------------------|--------------------------------------------------|
| 400  | `invalid_notification_method` | `method` wasn't one of the four known values   |
| 401  | `invalid_session`             | Token unknown / expired / `exe_changed`        |
| 403  | `scope_denied`                | Token lacks the `settings` scope               |

## `GET /settings/notification_method`

**Request:**

```http
GET /settings/notification_method HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":              "success",
  "notification_method": "auto"
}
```

**Errors:**

| HTTP | `message`         | Meaning                                        |
|------|-------------------|--------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`        |
| 403  | `scope_denied`    | Token lacks the `settings` scope               |
