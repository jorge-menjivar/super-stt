# `GET /status`

Snapshot of the daemon's current operational state — which model is
loaded and which device it's running on. Subscriber introspection
and other operator info are not exposed here; for those, the
`settings` scope's [`GET /active_model`](./active_model.md) and
[`GET /active_device`](./active_device.md) endpoints apply.

## Auth

- **Required scope:** `status`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `status` scope get `403 scope_denied`.

## `GET /status`

**Request:**

```http
GET /status HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":        "success",
  "device":        "cuda",
  "model_loaded":  true,
  "current_model": "whisper-tiny",
  "busy":          false
}
```

| Field           | Type    | Notes                                                                                  |
|-----------------|---------|----------------------------------------------------------------------------------------|
| `device`        | string  | `"cpu"`, `"cuda"`, `"metal"`, or `"unknown"` if nothing is loaded                       |
| `model_loaded`  | bool    | `false` while the daemon is still loading the initial model or after a failed switch   |
| `current_model` | string? | The loaded model's name (e.g. `whisper-tiny`); absent when `model_loaded` is `false`   |
| `busy`          | bool    | `true` while a daemon-mic cycle is active — covers audio capture **and** the post-capture transcription/typing. Clients implementing a toggle hotkey consult this and call [`POST /transcribe/stop`](./transcribe/stop.md) when `true`, [`POST /transcribe`](./transcribe.md) when `false`. |

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed` — re-auth and retry   |
| 403  | `scope_denied`    | Token lacks the `status` scope                               |
