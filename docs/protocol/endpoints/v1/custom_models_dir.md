# `/custom_models_dir`

Read and set the filesystem path that gets scanned for
user-supplied STT models. After a successful `POST`, the new
directory is scanned immediately and any discovered models become
selectable via [`GET /models`](./models.md) (with `source:
"custom"`) and switchable via
[`POST /active_model`](./active_model.md).

Pass `path: null` (or omit it) to clear the override.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /custom_models_dir`

**Request:**

```http
POST /custom_models_dir HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "path": "/home/u/super-stt-models"
}
```

| Field  | Type     | Required | Notes                                                                              |
|--------|----------|----------|------------------------------------------------------------------------------------|
| `path` | string?  | no       | Absolute path to a directory readable by the daemon. `null` (or omitted) clears it. |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":  "success",
  "message": "Custom models directory set to /home/u/super-stt-models"
}
```

After the response, calling [`GET /models`](./models.md) reflects
any models newly discovered under the path; entries from the path
carry `source: "custom"`.

**Errors:**

| HTTP | `message`                  | Meaning                                                       |
|------|----------------------------|---------------------------------------------------------------|
| 400  | `invalid_custom_models_dir`| Path is empty / not absolute / not readable                   |
| 401  | `invalid_session`          | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`             | Token lacks the `settings` scope                              |

## `GET /custom_models_dir`

**Request:**

```http
GET /custom_models_dir HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200, override set):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":            "success",
  "custom_models_dir": "/home/u/super-stt-models"
}
```

**Response (200, no override):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":            "success",
  "custom_models_dir": null
}
```

| Field               | Type     | Notes                                          |
|---------------------|----------|------------------------------------------------|
| `custom_models_dir` | string?  | `null` when no override is configured          |

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Token lacks the `settings` scope                              |
