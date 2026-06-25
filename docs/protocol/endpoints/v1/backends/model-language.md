# `/backends/{source}/models/{model}/language`

Read and set the **per-model language override** for any backend model (loaded
or not), and read the daemon's resolved effective language. The override is one
of the model's `supported_languages`, the reserved `auto`, or absent (Automatic
— inherits the global [`/language`](../../language.md), else the model's
`primary_language`). It is stored per model and survives model switches. Only
multilingual models accept an override.

`{source}` is the backend's repo id (e.g. `github.com/super-stt/whisper`),
**URL-percent-encoded** in the path — the same identifier used by
[`DELETE /backends/{source}`](../../backends.md#delete-backendssource):

```
/backends/github.com%2Fsuper-stt%2Fwhisper/models/whisper-large-v3/language
```

`{model}` is the model name as it appears in the backend's `models` array (see
[`GET /backends`](../../backends.md)); model names contain only alphanumerics
and hyphens, so the segment is used as-is (no encoding needed).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required on every request.
- A token without the `settings` scope gets `403 scope_denied`.

## `GET /backends/{source}/models/{model}/language`

Returns the daemon's full resolution for the named model.

**Request:**

```http
GET /backends/github.com%2Fsuper-stt%2Fwhisper/models/whisper-large-v3/language HTTP/1.1
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

## `POST /backends/{source}/models/{model}/language`

**Request:**

```http
POST /backends/github.com%2Fsuper-stt%2Fwhisper/models/whisper-large-v3/language HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{ "language": "es-419" }
```

| Field      | Type   | Required | Notes                                                              |
|------------|--------|----------|--------------------------------------------------------------------|
| `language` | string | yes      | One of the model's `supported_languages`, or `auto`. To clear, `DELETE`. |

**Response (200):** the resolution block (as `GET`).

## `DELETE /backends/{source}/models/{model}/language`

Clear the override (back to Automatic).

**Request:**

```http
DELETE /backends/github.com%2Fsuper-stt%2Fwhisper/models/whisper-large-v3/language HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):** the resolution block (as `GET`).

## Errors

| HTTP | `message`              | Meaning                                                                                                      |
|------|------------------------|--------------------------------------------------------------------------------------------------------------|
| 400  | `unsupported_language` | `language` is not in the model's `supported_languages` (and isn't `auto`), or the model is not multilingual |
| 401  | `invalid_session`      | Token unknown / expired / `exe_changed`                                                                      |
| 403  | `scope_denied`         | Token lacks the `settings` scope                                                                             |
| 404  | `unknown_backend`      | No installed backend has that `source`                                                                       |
| 404  | `unknown_model`        | `{model}` is not served by that backend                                                                      |
