# `/active_model/language`

Read and set the **per-model language override** for the currently active model,
and read the daemon's resolved effective language. The override is one of the
model's `supported_languages`, the reserved `auto`, or absent (Automatic —
inherit the global [`/language`](../language.md), else the model's
`primary_language`). It is stored per model and survives model switches. Only
multilingual models accept an override.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `GET /active_model/language`

Returns the daemon's full resolution for the active model.

**Request:**

```http
GET /active_model/language HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{
  "status": "success",
  "language": {
    "multilingual": true,
    "source":    "override",      // "override" | "global" | "default"
    "effective": "es-419",        // the value sent to the backend; null = omitted (model primary)
    "override":  "es-419",        // the stored per-model value, or null
    "primary":   "en",            // the model's primary_language (the fallback)
    "supported": ["en", "es-419", "es-ES", "fr"]
  }
}
```

For a non-multilingual model: `"multilingual": false`, `"supported": ["en"]`,
`"effective": null`, `"override": null`, `"primary": "en"`, `"source": "default"`.

## `POST /active_model/language`

**Request:**

```http
POST /active_model/language HTTP/1.1
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{ "language": "es-419" }
```

| Field      | Type   | Required | Notes                                                              |
|------------|--------|----------|--------------------------------------------------------------------|
| `language` | string | yes      | One of the model's `supported_languages`, or `auto`. To clear, `DELETE`. |

**Response (200):** the resolution block (as `GET`).

## `DELETE /active_model/language`

Clear the override (back to Automatic).

**Request:**

```http
DELETE /active_model/language HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):** the resolution block (as `GET`).

**Errors (all methods):**

| HTTP | `message`               | Meaning                                                  |
|------|-------------------------|----------------------------------------------------------|
| 400  | `unsupported_language`  | `language` is not in the active model's `supported_languages` (and isn't `auto`), or the model is not multilingual |
| 401  | `invalid_session`       | Token unknown / expired / `exe_changed`                  |
| 403  | `scope_denied`          | Token lacks the `settings` scope                         |
| 409  | `not_ready`             | No model is active                                       |
