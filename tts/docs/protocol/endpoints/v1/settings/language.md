# `/settings/language`

Read and set the **global Primary Language** — the default speech
language applied to any multilingual model that supports it. A value is a
BCP-47 tag (`en`, `es-MX`, `es-419`) or the reserved `auto` (auto-detect);
absent means no preference (each model uses its own `primary_language`). Per-model
overrides live at [`/pipeline/{stage}/model/{model}/language`](../pipeline/language.md).

This setting is the middle term of a three-step resolution: a model's own
override wins, then this value, then the model's `primary_language`. Setting it
here spares a user who speaks one language from pinning every model
individually, and a model that cannot speak the chosen tag ignores it rather
than failing — the resolution each model actually arrives at is what
[`GET /pipeline/{stage}/model/{model}/language`](../pipeline/language.md#get-pipelinestagemodelmodellanguage)
reports.

What this endpoint accepts is [`GET /settings/language/list`](./language/list.md),
kept separate for the same reason every "is set" / "may be set" pair on this
surface is: only one of the two changes when the user picks a language.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /settings/language`

**Request:**

```http
POST /settings/language HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
Content-Type: application/json

{
  "language": "es-MX"
}
```

| Field      | Type   | Required | Notes                                          |
|------------|--------|----------|------------------------------------------------|
| `language` | string | yes      | A tag [`/settings/language/list`](./language/list.md) offers, or `auto`. Any other is refused. To clear, use `DELETE`. |

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
Host: tts.local
Authorization: Bearer tts_…64hex…
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
Host: tts.local
Authorization: Bearer tts_…64hex…
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

**Errors (`POST`):**

| HTTP | `error_code`           | Meaning                                                  |
|------|------------------------|----------------------------------------------------------|
| 400  | `unsupported_language` | `language` is not one `/settings/language/list` offers |
