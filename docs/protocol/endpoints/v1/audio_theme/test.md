# `POST /audio_theme/test`

Play the current theme's start + stop cues so the user can audition
their selection before saving. The theme itself is read / set via
[`/audio_theme`](../audio_theme.md). Volume is controlled by
[`/volume`](../volume.md).

This endpoint produces sound on the host the daemon is running on —
not on the client. Calling it from a remote / sandboxed client does
nothing the user can hear.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- `client` / `widget` tokens get `403 scope_denied`.

## `POST /audio_theme/test`

**Request:**

```http
POST /audio_theme/test HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

No request body.

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":  "success",
  "message": "Theme tested successfully"
}
```

The call returns once the cue sequence has been queued for playback;
the audio itself plays asynchronously on the daemon host. Playback
length depends on the current theme.

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Not a `settings` token                                        |
| 500  | `playback_failed` | The cue sequence could not be played (no audio device, etc.)  |
