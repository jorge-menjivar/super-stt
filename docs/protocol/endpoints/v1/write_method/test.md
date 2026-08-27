# `POST /write_method/test`

Type a short fixed string using the configured write method, so a
settings UI can show the user whether keyboard simulation actually
reaches their focused window. The method itself is read / set via
[`/write_method`](../write_method.md).

The string typed is `Super STT input test 123`.

The text goes to whatever window holds keyboard focus on the daemon
host at the moment of the call — the daemon cannot target a specific
window, and takes no delay parameter. A client offering this as a
button has two useful shapes: focus a text field of its own first, so
the user has somewhere to see the result; or count down before
calling, so the user can switch to the window they actually dictate
into. The second is the stronger check, since an app that accepts
simulated keys in one client may still drop them in another.
Calling it from a remote / sandboxed client types into the *daemon
host's* focused window, not the caller's.

A `success` response means the backend accepted the keystrokes, not
that they appeared on screen. Some Wayland input paths report success
even when the compositor routes the events nowhere — for instance
when no text input is active in the focused client. Seeing the text
is the actual test; the response only rules out the failures the
daemon can detect.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /write_method/test`

**Request:**

```http
POST /write_method/test HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

No request body.

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":                "success",
  "message":               "Typed test text via Wayland protocol",
  "write_method":          "auto",
  "resolved_write_method": "wayland_protocol"
}
```

| Field                   | Type   | Notes                                                                                                   |
|-------------------------|--------|---------------------------------------------------------------------------------------------------------|
| `write_method`          | string | The configured method, exactly as [`GET /write_method`](../write_method.md) reports it.                 |
| `resolved_write_method` | string | The backend that typed: `xdg_desktop_portal`, `ydotool`, or `wayland_protocol` — never `auto`.          |

`resolved_write_method` is the useful half when `write_method` is
`auto`: it names the rung the auto chain settled on, which is
otherwise not observable from the protocol.

The call returns once the keystrokes have been delivered to the
backend, so a long string blocks for as long as typing takes.

**Errors:**

| HTTP | `message`                  | Meaning                                                                       |
|------|----------------------------|---------------------------------------------------------------------------------|
| 401  | `invalid_session`          | Token unknown / expired / `exe_changed`                                        |
| 403  | `scope_denied`             | Token lacks the `settings` scope                                              |
| 409  | `recording_in_progress`    | A daemon-mic recording is active and already owns the keyboard                 |
| 500  | `write_method_unavailable` | No backend could be built — for `auto`, every method in the chain was unusable |
| 500  | `typing_failed`            | A backend was built but the keystrokes could not be delivered                  |
