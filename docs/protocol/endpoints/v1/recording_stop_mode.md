# `/recording_stop_mode`

Read and set the default stop behavior for
[`POST /transcribe`](./transcribe.md) when no per-request
`stop_mode` is sent.

| Mode                 | Stops on silence (VAD) | Stops on manual signal                      |
|----------------------|------------------------|---------------------------------------------|
| `silence_only`            | yes                    | no                                          |
| `silence_and_manual` | yes                    | yes                                         |
| `manual_only`        | no                     | yes — explicit [`/transcribe/stop`](./transcribe/stop.md) or socket disconnect |

Per-request overrides on `/transcribe` take precedence over the
default set here.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /recording_stop_mode`

**Request:**

```http
POST /recording_stop_mode HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "mode": "manual_only"
}
```

| Field  | Type   | Required | Notes                                                       |
|--------|--------|----------|-------------------------------------------------------------|
| `mode` | string | yes      | One of `silence_only`, `silence_and_manual`, `manual_only`. An unrecognized value falls back to the default (`silence_and_manual`) rather than being rejected. |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":              "success",
  "recording_stop_mode": "manual_only"
}
```

**Errors:**

| HTTP | `message`                    | Meaning                                                       |
|------|------------------------------|---------------------------------------------------------------|
| 400  | (request error)              | `mode` was absent from the request body                        |
| 401  | `invalid_session`            | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`               | Token lacks the `settings` scope                              |

A **present but unrecognized** `mode` value is **not** an error — it falls back to
the default (`silence_and_manual`), matching the wire-enum house rule used across
the protocol and the `record` command's `stop_mode` override. Only an *absent*
`mode` field is a client error.

## `GET /recording_stop_mode`

**Request:**

```http
GET /recording_stop_mode HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":              "success",
  "recording_stop_mode": "silence_and_manual"
}
```

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Token lacks the `settings` scope                             |
