# `/audio_theme`

Read and set the audio cue theme that plays on recording
start/stop. The catalog of available themes lives at
[`GET /audio_themes`](./audio_themes.md); to actually play the
current theme's cues (e.g. as a preview in a settings UI), use
[`POST /audio_theme/test`](./audio_theme/test.md).

Themes are named strings. The current set is `classic`, `gentle`,
`minimal`, `scifi`, `musical`, `nature`, `retro`, `silent`. The
canonical list at any moment is what
[`GET /audio_themes`](./audio_themes.md) returns.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /audio_theme`

**Request:**

```http
POST /audio_theme HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "theme": "scifi"
}
```

| Field   | Type   | Required | Notes                                                                  |
|---------|--------|----------|------------------------------------------------------------------------|
| `theme` | string | yes      | One of the names returned by [`GET /audio_themes`](./audio_themes.md)  |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":      "success",
  "audio_theme": "scifi"
}
```

**Errors:**

| HTTP | `message`             | Meaning                                                       |
|------|-----------------------|---------------------------------------------------------------|
| 400  | `invalid_audio_theme` | Unknown theme name                                            |
| 401  | `invalid_session`     | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`        | Token lacks the `settings` scope                              |

## `GET /audio_theme`

**Request:**

```http
GET /audio_theme HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":      "success",
  "audio_theme": "classic"
}
```

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Token lacks the `settings` scope                              |
