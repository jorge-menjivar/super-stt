# `/volume`

Read and set the audio cue volume on a 0–100 scale. `0` mutes the
cues without changing the active theme; `100` is full. The theme
itself is read / set via [`/audio_theme`](./audio_theme.md).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- `client` / `widget` tokens get `403 scope_denied`.

## `POST /volume`

**Request:**

```http
POST /volume HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "volume": 75
}
```

| Field    | Type | Required | Notes                              |
|----------|------|----------|------------------------------------|
| `volume` | u8   | yes      | Integer in `0..=100`               |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":  "success",
  "message": "Volume set to 75"
}
```

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 400  | `invalid_volume`  | `volume` outside `0..=100`                                    |
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Not a `settings` token                                        |

## `GET /volume`

**Request:**

```http
GET /volume HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":  "success",
  "message": "Volume is 75"
}
```

The `message` carries the current volume as a sentence. The
preferred shape is the integer in `0..=100` itself; UIs should
parse the number out of the message rather than displaying it raw.

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Not a `settings` token                                        |
