# `/write_method`

Read and set the keyboard simulation method used when a
[`POST /transcribe`](./transcribe.md) request has `write_mode:
true` and needs to type the final transcription into the focused
window.

| Method                | Notes                                                                                |
|-----------------------|--------------------------------------------------------------------------------------|
| `auto`                | Pick the best available method for the current session (the default).               |
| `xdg-desktop-portal`  | Use the portal's `RemoteDesktop` interface; requires a portal available on the bus.  |
| `ydotool`             | Use the `ydotool` daemon if present; works without a portal on most Wayland sessions. |
| `wayland-protocol`    | Use a direct Wayland protocol path (compositor-dependent).                            |

The new method takes effect on the next `/transcribe` request.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- `client` / `widget` tokens get `403 scope_denied`.

## `POST /write_method`

**Request:**

```http
POST /write_method HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "method": "ydotool"
}
```

| Field    | Type   | Required | Notes                                                                          |
|----------|--------|----------|--------------------------------------------------------------------------------|
| `method` | string | yes      | One of `auto`, `xdg-desktop-portal`, `ydotool`, `wayland-protocol`             |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":       "success",
  "write_method": "ydotool"
}
```

**Errors:**

| HTTP | `message`              | Meaning                                                       |
|------|------------------------|---------------------------------------------------------------|
| 400  | `invalid_write_method` | `method` wasn't one of the four known values                  |
| 401  | `invalid_session`      | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`         | Not a `settings` token                                        |

## `GET /write_method`

**Request:**

```http
GET /write_method HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":       "success",
  "write_method": "auto"
}
```

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Not a `settings` token                                        |
