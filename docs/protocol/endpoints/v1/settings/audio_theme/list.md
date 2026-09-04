# `GET /settings/audio_theme/list`

List the audio cue themes available for selection via
[`POST /settings/audio_theme`](../audio_theme.md#post-settingsaudio_theme).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `GET /settings/audio_theme/list`

**Request:**

```http
GET /settings/audio_theme/list HTTP/1.1
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
| `available_audio_themes` | string[] | Stable theme names; one of these is what `POST /settings/audio_theme` accepts |

`silent` is a real theme that produces no sound — distinct from
muting via [`POST /settings/volume`](../volume.md) (volume `0` mutes the
selected theme without changing which theme is active).

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Token lacks the `settings` scope                              |
