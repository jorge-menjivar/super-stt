# `GET /status`

Snapshot of the daemon's current operational state, one part per feature:
which model each has loaded, which device it runs on, and whether it is at
work. Subscriber introspection and other operator info are not exposed here;
for those, the `settings` scope's [`GET /stt/pipeline/1`](./pipeline/stage.md),
[`GET /tts/pipeline/1`](./tts/pipeline/stage.md) and their `device`
endpoints apply.

## Auth

- **Required scope:** `status`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `status` scope get `403 scope_denied`.

## `GET /status`

**Request:**

```http
GET /status HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "success",
  "stt": {
    "device":        "cuda",
    "model_loaded":  true,
    "current_model": "whisper-tiny",
    "busy":          false
  },
  "tts": {
    "device":        "unknown",
    "model_loaded":  false,
    "busy":          false
  }
}
```

`stt` is speech to text's part and `tts` text to speech's. Both are always
present, with the same fields:

| Field           | Type    | Notes                                                                                  |
|-----------------|---------|----------------------------------------------------------------------------------------|
| `device`        | string  | The accelerator the feature's loaded model is actually running on: `"cpu"`, `"cuda"`, `"rocm"`, `"metal"`, `"vulkan"`, or `"remote"` for an online model. `"unknown"` if nothing is loaded. |
| `model_loaded`  | bool    | `false` while the feature's model is still loading, after a failed switch, or when none is selected |
| `current_model` | string? | The loaded model's name (e.g. `whisper-tiny`); absent when `model_loaded` is `false`   |
| `busy`          | bool    | Whether the feature is at work right now. For `stt`, `true` while a daemon-mic cycle is active — audio capture **and** the transcription and typing after it; a toggle hotkey consults it and calls [`POST /stt/transcribe/stop`](./transcribe/stop.md) when `true`, [`POST /stt/transcribe`](./transcribe.md) when `false`. For `tts`, `true` while an utterance is being synthesized or played; a [`POST /tts/speak`](./tts/speak.md) then interrupts it. |

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed` — re-auth and retry   |
| 403  | `scope_denied`    | Token lacks the `status` scope                               |
