# `/settings/notification_method`

Read and set how the daemon surfaces a recording or synthesis failure to the
user. One setting covers both features; how text to speech reads it is under
[Text to speech](#text-to-speech).

A failure — no model loaded, the recorder could not start, capture died
partway, or the model could not transcribe the audio — is reported to the
caller regardless of this setting: as the direct error response (e.g. `409
model_not_loaded`) or, once an SSE stream has started, via the `error` event.
This setting controls the additional, human-facing notice.

| Method    | Notes                                                                                          |
|-----------|------------------------------------------------------------------------------------------------|
| `auto`    | Send a desktop notification; if it cannot be delivered, type the notice instead (the default). |
| `desktop` | Send a desktop notification only. If it cannot be delivered, the failure is logged and nothing is shown. |
| `typed`   | Type a fixed notice into the focused window.                                                   |
| `off`     | Log the failure only; never surface it.                                                        |

`desktop` was called `dbus` before macOS support; the name now describes what
the user sees rather than one platform's transport for it. Neither this
endpoint nor the config file accepts the old token.

Desktop notifications use the freedesktop Desktop Notifications interface
(`org.freedesktop.Notifications`) on the session bus on Linux, so they work on
any desktop that provides a notification server, and Notification Center on
macOS.

**Two caveats on macOS.** A banner carries no action buttons, so the "Open
Super Voice" action on an update notice is absent there. And the daemon cannot
tell whether a banner was actually shown — if notifications are muted for the
posting application, or Do Not Disturb is on, the post still reports success,
so `auto` will *not* fall back to typing. Both follow from the daemon being a
plain binary rather than a bundled, signed `.app`.

Typing requires a `POST /stt/transcribe` with `write_mode: true`. For a recording
that is not in write mode, `typed` — and `auto` once notification delivery has
failed — logs the failure instead.

The new method takes effect on the next recording.

## Text to speech

A synthesis failure — no model loaded, the output device could not be opened,
or the backend produced no usable audio — is reported to the caller
regardless of this setting: as the direct error response (e.g. `409
model_not_loaded`) or, on a [`/tts/speak/stream`](../tts/speak/stream.md)
session, as an `error` frame.

Speech has one channel for the notice, a desktop notification, so the four
values come down to two:

1. `auto`, `desktop` and `typed` send a desktop notification, and log the
   failure if none can be reached. There is no field to type into for an
   utterance.
2. `off` logs the failure only.

A notice is raised only for failures the caller did not cause. Bad input —
missing or over-long `text` — gets the coded `400` and nothing else, because
whoever sent it is by definition looking at the response.

The new method takes effect on the next utterance.

## What the user sees

The two channels carry different amounts of detail, because they land in
different places.

A notification has a summary and a body of its own, and its app name and icon
are supplied separately — so the summary names the failure and the body gives
the reason:

| Failure                                | Summary                     | Body                             |
|----------------------------------------|-----------------------------|----------------------------------|
| No model is loaded                     | `No model loaded`           | `Load a model and try again.`    |
| The selected model failed to load      | `Could not load the model`  | The reason, from the daemon      |
| The recorder could not start           | `Could not start recording` | The reason, from the daemon      |
| Capture died partway                   | `Recording failed`          | The reason, from the daemon      |
| The audio was not transcribed          | `Transcription failed`      | The reason, from the backend     |
| The output device could not be opened  | `Could not play audio`      | The reason, from the daemon      |
| The backend produced no audio          | `Synthesis failed`          | The reason, from the backend     |

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
[Super Voice: no model loaded]
[Super Voice: could not load the model]
[Super Voice: could not start recording]
[Super Voice: recording failed]
[Super Voice: transcription failed]
```

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /settings/notification_method`

**Request:**

```http
POST /settings/notification_method HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
Content-Type: application/json

{
  "method": "desktop"
}
```

| Field    | Type   | Required | Notes                                          |
|----------|--------|----------|--------------------------------------------------|
| `method` | string | yes      | One of `auto`, `desktop`, `typed`, `off`       |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":              "success",
  "notification_method": "desktop"
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
Host: voice.local
Authorization: Bearer …64hex…
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
