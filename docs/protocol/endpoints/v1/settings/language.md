# `/settings/language`

Read and set the **global Primary Language** — the default transcription
language applied to any multilingual model that supports it. A value is a
BCP-47 tag (`en`, `es-MX`, `es-419`) or the reserved `auto` (auto-detect);
absent means no preference (each model uses its own `primary_language`). Per-model
overrides live at [`/backends/{source}/models/{model}/settings/language`](../pipeline/language.md).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /settings/language`

**Request:**

```http
POST /settings/language HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "language": "es-MX"
}
```

| Field      | Type   | Required | Notes                                          |
|------------|--------|----------|------------------------------------------------|
| `language` | string | yes      | A BCP-47 tag or `auto`. To clear, use `DELETE`. |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{ "status": "success", "language": "es-MX" }
```

## `GET /settings/language`

**Request:**

```http
GET /settings/language HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{ "status": "success", "language": "es-MX" }
```

`language` is the configured tag, `"auto"`, or `null` when unset.

## `DELETE /settings/language`

Clear the global Primary Language (back to no preference).

**Request:**

```http
DELETE /settings/language HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{ "status": "success", "language": null }
```

**Errors (all methods):**

| HTTP | `message`         | Meaning                                 |
|------|-------------------|-----------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed` |
| 403  | `scope_denied`    | Token lacks the `settings` scope        |
