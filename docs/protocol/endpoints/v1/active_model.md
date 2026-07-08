# `/active_model`

Read and switch the active STT model. Cancellation of an in-flight
switch lives at [`POST /active_model/cancel`](./active_model/cancel.md);
the catalog of available models lives at [`GET /models`](./models.md).

The active model is identified by a `(name, provider, source)`
triple:

- **`name`** — `whisper-1`, `voxtral-mini`, …
- **`provider`** — `local_whisper`, `local_voxtral`, `openai`,
  `mistral`, or `deepgram`.
- **`source`** — the repo id of the backend that serves the model
  (e.g. `github.com/super-stt/openai`), as returned by
  [`GET /models`](./models.md). Empty/omitted selects the first installed
  backend serving `(name, provider)`.

Switching the model also sets the [active backend](./active_backend.md) to the
model's `source`. If the model then fails to load (e.g. a missing secret), the
backend stays selected but **no model is loaded** — the daemon does not silently
restore a previously-loaded model. Clear the selection with
[`DELETE /active_backend`](./active_backend.md).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /active_model`

Start switching the active model. The call returns `202 Accepted`
immediately; the actual switch runs asynchronously. Progress is
visible via:

- [`GET /active_model`](#get-active_model) — polling.
- [`GET /events?topics=daemon_status_changed,download_progress`](./events.md)
  — push-based via SSE (recommended).

**Request:**

```http
POST /active_model HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "model":    "whisper-1",
  "provider": "openai",
  "source":   "github.com/super-stt/openai"
}
```

| Field      | Type    | Required | Notes                                                                  |
|------------|---------|----------|------------------------------------------------------------------------|
| `model`    | string  | yes      | One of the names returned by [`GET /models`](./models.md)              |
| `provider` | string  | yes      | One of `local_whisper`, `local_voxtral`, `openai`, `mistral`, `deepgram` |
| `source`   | string  | no       | Repo id of the serving backend. Empty/omitted picks the first backend serving `(model, provider)`. |

**Response (202):**

```http
HTTP/1.1 202 Accepted
Content-Type: application/json

{
  "status":  "success",
  "message": "Model switch started"
}
```

The actual identity / load state of the new model becomes visible
through [`GET /active_model`](#get-active_model) once the switch
completes; the most reliable client UX is to subscribe to the SSE
topics above.

**Errors:**

| HTTP | `message`                  | Meaning                                                                       |
|------|----------------------------|-------------------------------------------------------------------------------|
| 400  | `online_models_disabled`   | `provider` is online but [`allow_online_models`](./allow_online_models.md) is `false` |
| 400  | `invalid_model`            | No installed backend serves `(model, provider, source)`                       |
| 401  | `invalid_session`          | Token unknown / expired / `exe_changed`                                       |
| 403  | `scope_denied`             | Token lacks the `settings` scope                                              |
| 409  | `switch_in_progress`       | Another model switch is already running                                       |
| 409  | `recording_in_progress`    | A recording is active; cancel or finish it before switching                   |

## `GET /active_model`

Returns the currently-active model plus the state of any in-flight
switch, in a single payload so a settings UI can render the entire
"Model" section from one request.

**Request:**

```http
GET /active_model HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{
  "status": "success",
  "active_model": {

    // Always present — what's currently active right now.
    // Reflects the previously-loaded model while a switch is in
    // flight, and the new model once that switch succeeds.
    "current": {
      "model":    "voxtral-mini",
      "provider": "local_voxtral",
      "source":   "github.com/super-stt/voxtral",
      "loaded":   true,
      "device":   "cuda"            // "cpu" / "cuda" / "metal" / "remote"
    },

    // Present only when a download is in flight. `null` otherwise.
    // For the "is the model ready?" signal, watch the
    // `daemon_status_changed` SSE topic for status: "ready".
    "switch": {
      "phase":      "downloading",  // "downloading" | "completed"
                                    // | "cancelled" | "error"
      "target":     { "model": "whisper-base" },
      "started_at": "2026-05-22T12:00:00Z",
      "download": {
        "current_file":     "model.safetensors",
        "file_index":       1,
        "total_files":      3,
        "bytes_downloaded": 12345678,
        "total_bytes":      45678901,
        "percentage":       27.0,
        "eta_seconds":      14
      }
    }
  }
}
```

**Phase values** (`switch.phase`):

| Value          | Meaning                                                                              |
|----------------|--------------------------------------------------------------------------------------|
| `downloading`  | Pulling model files; `download` sub-object is populated.                              |
| `completed`    | Downloads finished; the model is being loaded onto the chosen device.                 |
| `cancelled`    | [`POST /active_model/cancel`](./active_model/cancel.md) interrupted the download.    |
| `error`        | Download failed (network, disk, hash mismatch, …).                                    |

`switch` is `null` when no download is in flight.

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Token lacks the `settings` scope                              |

## `DELETE /active_model`

Unload the currently loaded model. The active backend stays selected — the
user can immediately pick another of its models with `POST /active_model`.
To return the daemon to fully idle, use [`DELETE /active_backend`](./active_backend.md)
instead. No-op when nothing is loaded; rejected during an active recording or
real-time transcription session.

**Request:**

```http
DELETE /active_model HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{ "status": "success", "message": "Unloaded whisper-1" }
```

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 409  | `recording_in_progress` | A recording or real-time session is active; stop it first |
| 401  | `invalid_session`       | Token unknown / expired / `exe_changed`                  |
| 403  | `scope_denied`          | Token lacks the `settings` scope                         |
