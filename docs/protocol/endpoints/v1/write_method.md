# `/write_method`

Read and set the keyboard simulation method used when a
[`POST /transcribe`](./transcribe.md) request has `write_mode:
true` and needs to type the final transcription into the focused
window.

| Method               | Notes                                                                                                                    |
|----------------------|--------------------------------------------------------------------------------------------------------------------------|
| `auto`               | Try `wayland_protocol`, then `xdg_desktop_portal`, then `ydotool`, and use the first the session supports (the default). |
| `xdg_desktop_portal` | Use the portal's `RemoteDesktop` interface; requires a portal exporting it on the session bus.                            |
| `ydotool`            | Use the `ydotool` daemon if present; works without a portal on most Wayland sessions.                                    |
| `wayland_protocol`   | Use a direct Wayland protocol path; requires a compositor exposing `zwp_virtual_keyboard_manager_v1`.                     |

A specific method is used as given: when it is unavailable the request that
needs it fails rather than falling back. Only `auto` walks the chain.

The new method takes effect on the next `/transcribe` request. To
check that the configured method can actually type — and, for `auto`,
to learn which backend it resolves to — use
[`POST /write_method/test`](./write_method/test.md).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

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
| `method` | string | yes      | One of `auto`, `xdg_desktop_portal`, `ydotool`, `wayland_protocol`             |

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
| 403  | `scope_denied`         | Token lacks the `settings` scope                              |

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
| 403  | `scope_denied`    | Token lacks the `settings` scope                              |
