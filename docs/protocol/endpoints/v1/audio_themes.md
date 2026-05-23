# `GET /audio_themes`

List the audio cue themes available for selection via
[`POST /audio_theme`](./audio_theme.md#post-audio_theme).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- `client` / `widget` tokens get `403 scope_denied`.

## `GET /audio_themes`

**Request:**

```http
GET /audio_themes HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":                 "success",
  "available_audio_themes": [
    "classic",
    "gentle",
    "minimal",
    "scifi",
    "musical",
    "nature",
    "retro",
    "silent"
  ]
}
```

| Field                    | Type     | Notes                                                                |
|--------------------------|----------|----------------------------------------------------------------------|
| `available_audio_themes` | string[] | Stable theme names; one of these is what `POST /audio_theme` accepts |

`silent` is a real theme that produces no sound — distinct from
muting via [`POST /volume`](./volume.md) (volume `0` mutes the
selected theme without changing which theme is active).

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Not a `settings` token                                        |
