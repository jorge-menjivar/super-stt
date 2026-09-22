# `/settings/write_method`

Read and set the keyboard simulation method used when a
[`POST /transcribe`](../transcribe.md) request has `write_mode:
true` and needs to type the final transcription into the focused
window.

| Method               | Platform    | Notes                                                                                                                        |
|----------------------|-------------|------------------------------------------------------------------------------------------------------------------------------|
| `auto`               | all         | Use the first method the session supports (the default). Linux walks `built_in` → `xdg_desktop_portal` → `ydotool`; macOS has only `built_in`. |
| `built_in`           | all         | The daemon types the text itself, with no helper process to install. On Linux this needs a compositor exposing `zwp_virtual_keyboard_manager_v1`; on macOS it posts CoreGraphics key events and needs the Accessibility permission. |
| `xdg_desktop_portal` | Linux only  | Use the portal's `RemoteDesktop` interface; requires a portal exporting it on the session bus.                                |
| `ydotool`            | Linux only  | Use the `ydotool` daemon if present; works without a portal on most Wayland sessions.                                        |

`built_in` was called `wayland_protocol` before macOS support; the name now
describes what the user is choosing rather than one platform's mechanism for
it. The old token is not accepted — a stored `wayland_protocol` falls back to
`auto`, which resolves to the same backend on Linux.

A specific method is used as given: when it is unavailable the request that
needs it fails rather than falling back. Only `auto` walks the chain. Asking
for a Linux-only method on macOS is rejected outright rather than reported as
temporarily unavailable.

The new method takes effect on the next `/transcribe` request. To
check that the configured method can actually type — and, for `auto`,
to learn which backend it resolves to — use
[`POST /settings/write_method/test`](./write_method/test.md).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /settings/write_method`

**Request:**

```http
POST /settings/write_method HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "method": "ydotool"
}
```

| Field    | Type   | Required | Notes                                                                          |
|----------|--------|----------|--------------------------------------------------------------------------------|
| `method` | string | yes      | One of `auto`, `built_in`, `xdg_desktop_portal`, `ydotool`                     |

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

## `GET /settings/write_method`

**Request:**

```http
GET /settings/write_method HTTP/1.1
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
