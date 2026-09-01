# `GET /models`

List the models the settings UI may switch to. Models are served by
out-of-tree backends discovered on disk; each entry is identified by the
pair `(name, source)`, where `source` is the repo id of the
backend that serves it (see [`docs/protocol/backend/`](../../backend/)).

The list is **scoped to the [active backend](./pipeline.md)** — it returns
only the models served by the currently-selected backend, and is empty when no
backend is selected (daemon idle). The full catalog of installed backends and
their models is available from [`GET /backends`](./backends.md).

Only **transcription** models are listed. A backend's
[post-processor](./pipeline.md) models are excluded, since they cannot be
switched to here — they are selected through
[`/pipeline/2`](./pipeline.md) instead. `GET /backends` lists every
model with its `role`.

Selecting one is done via
[`POST /pipeline/1/model`](./pipeline.md#post-pipelinestagemodel).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `GET /models`

**Request:**

```http
GET /models HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{
  "status": "success",
  "available_models": [
    ["voxtral-mini", "github.com/super-stt/voxtral"],
    ["whisper-1", "github.com/super-stt/openai"]
  ]
}
```

| Field              | Type            | Notes                                                                 |
|--------------------|-----------------|-----------------------------------------------------------------------|
| `available_models` | array of arrays | Each entry is the `[name, source]` pair `POST /pipeline/1/model` accepts. `source` is the serving backend's repo id. |

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Token lacks the `settings` scope                              |
