# `/recording_stop_mode`

Read and set the default stop behavior for
[`POST /transcribe`](./transcribe.md) when no per-request
`stop_mode` is sent.

| Mode                 | Stops on silence (VAD) | Stops on manual signal                      |
|----------------------|------------------------|---------------------------------------------|
| `silence`            | yes                    | no                                          |
| `silence-and-manual` | yes                    | yes                                         |
| `manual-only`        | no                     | yes — explicit [`/transcribe/stop`](./transcribe/stop.md) or socket disconnect |

Per-request overrides on `/transcribe` take precedence over the
default set here.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- `client` / `widget` tokens get `403 scope_denied`.

## `POST /recording_stop_mode`

**Request:**

```http
POST /recording_stop_mode HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "mode": "manual-only"
}
```

| Field  | Type   | Required | Notes                                                       |
|--------|--------|----------|-------------------------------------------------------------|
| `mode` | string | yes      | One of `silence`, `silence-and-manual`, `manual-only`       |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":              "success",
  "recording_stop_mode": "manual-only"
}
```

**Errors:**

| HTTP | `message`                    | Meaning                                                       |
|------|------------------------------|---------------------------------------------------------------|
| 400  | `invalid_recording_stop_mode`| `mode` wasn't one of the three known values                   |
| 401  | `invalid_session`            | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`               | Not a `settings` token                                        |

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
  "recording_stop_mode": "silence-and-manual"
}
```

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Not a `settings` token                                        |
