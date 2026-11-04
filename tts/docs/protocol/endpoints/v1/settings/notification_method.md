# `/settings/notification_method`

Read and set how the daemon surfaces a synthesis failure to the user.

A failure — no model loaded, the output device could not be opened, or the
backend produced no usable audio — is reported to the caller regardless of this
setting: as the direct error response (e.g. `409 model_not_loaded`) or, on a
[`/speak/stream`](../speak/stream.md) session, as an `error` frame. This setting
controls the additional, human-facing notice.

| Method | Notes                                                                                                |
|--------|------------------------------------------------------------------------------------------------------|
| `auto` | Surface through the best channel available — today a desktop notification; logged if none can be reached (the default). |
| `off`  | Log the failure only; never surface it.                                                              |

Two values, not four: the STT build could also *type* a notice into the focused
window, which is why it distinguished "notify, else type" from "notify only".
With audio as the only output there is one channel left, and a setting whose
values behave identically is a trap. `auto` names the intent rather than the
mechanism, so a second channel can be added later without another wire value.

Desktop notifications use the freedesktop Desktop Notifications interface
(`org.freedesktop.Notifications`) on the session bus, so they work on any
desktop that provides a notification server.

The new method takes effect on the next utterance.

## What the user sees

A notification has a summary and a body of its own, and its app name and icon
are supplied separately — so the summary names the failure and the body gives
the reason:

| Failure                             | Summary                | Body                          |
|-------------------------------------|------------------------|-------------------------------|
| No model is loaded                  | `No model loaded`      | `Load a model and try again.` |
| The output device could not be opened | `Could not play audio` | The reason, from the daemon   |
| The backend produced no audio       | `Synthesis failed`     | The reason, from the backend  |

Reasons authored by a backend are prefixed `Backend error:`, so a failure the
daemon is only relaying is never mistaken for one of its own. Backend text is
untrusted: it is flattened to a single line, escaped so a notification server
that renders markup in the body cannot be driven from it, and clamped to 300
characters. A failure that arrives with no reason to report falls back to a
fixed sentence rather than an empty body.

A notice is raised only for failures the caller did not cause. Bad input —
missing or over-long `text` — gets the coded `400` and nothing else, because
whoever sent it is by definition looking at the response.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /settings/notification_method`

**Request:**

```http
POST /settings/notification_method HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
Content-Type: application/json

{
  "method": "off"
}
```

| Field    | Type   | Required | Notes                          |
|----------|--------|----------|--------------------------------|
| `method` | string | yes      | One of `auto`, `off`           |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":              "success",
  "notification_method": "off"
}
```

**Errors:**

| HTTP | `message`                     | Meaning                                        |
|------|-------------------------------|--------------------------------------------------|
| 400  | `invalid_notification_method` | `method` wasn't `auto` or `off`                |
| 401  | `invalid_session`             | Token unknown / expired / `exe_changed`        |
| 403  | `scope_denied`                | Token lacks the `settings` scope               |

## `GET /settings/notification_method`

**Request:**

```http
GET /settings/notification_method HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
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
