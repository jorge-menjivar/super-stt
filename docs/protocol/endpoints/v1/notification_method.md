# `/notification_method`

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

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /notification_method`

**Request:**

```http
POST /notification_method HTTP/1.1
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

## `GET /notification_method`

**Request:**

```http
GET /notification_method HTTP/1.1
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
