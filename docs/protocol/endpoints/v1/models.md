# `GET /models`

List every model the settings UI may switch to: built-in models
from the static registry plus any custom models discovered under
[`/custom_models_dir`](./custom_models_dir.md).

Selecting one is done via
[`POST /active_model`](./active_model.md#post-active_model).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- `client` / `widget` tokens get `403 scope_denied`.

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
    {
      "name":     "whisper-tiny",
      "provider": "local_whisper",
      "source":   "builtin"
    },
    {
      "name":     "voxtral-mini-latest",
      "provider": "local_voxtral",
      "source":   "builtin"
    },
    {
      "name":     "whisper-1",
      "provider": "openai",
      "source":   "online"
    },
    {
      "name":     "my-fine-tuned-whisper",
      "provider": "local_whisper",
      "source":   "custom"
    }
  ]
}
```

| Field              | Type           | Notes                                                                 |
|--------------------|----------------|-----------------------------------------------------------------------|
| `available_models` | array of objects | Each entry carries `name`, `provider`, `source` — the triple `POST /active_model` accepts |

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Not a `settings` token                                        |
