# `/shortcuts`

Super Voice's keyboard shortcuts on this desktop: where each one stands, adding
one, and removing one. The daemon owns them, so every client sees the same
answer, and a client that cannot write desktop settings, like a sandboxed app,
can still add one.

| `id`      | Does                                  | Its keys    | What it runs           |
|-----------|---------------------------------------|-------------|------------------------|
| `dictate` | Record, transcribe, and type the text | Super+Space | `voice record --write` |

How the daemon puts a shortcut on the desktop is the `method`:

| `method`  | Desktop                                     | What the daemon does                                                                                   |
|-----------|---------------------------------------------|--------------------------------------------------------------------------------------------------------|
| `cosmic`  | COSMIC                                      | Writes an entry to COSMIC's custom shortcuts, which runs the command.                                 |
| `portal`  | One with the `GlobalShortcuts` portal, such as KDE Plasma, GNOME or Hyprland | Binds it in a portal session it holds, and runs the command when it is pressed. |
| `manual`  | Any other                                   | Nothing. The user binds `command` in the desktop's own keyboard settings.                             |

Each shortcut's `state`:

| `state`     | Meaning                                                                                   |
|-------------|-------------------------------------------------------------------------------------------|
| `installed` | Set up, on `keys`. Those can be other keys than its own, if the user rebound it.         |
| `missing`   | Not set up, and `keys` are free. `POST /shortcuts/{id}` adds it.                         |
| `taken`     | Not set up, and another shortcut holds `keys`. The user binds `command` to other keys.   |
| `manual`    | The `manual` method: the user binds `command` in the desktop's settings.                 |

The daemon asks the desktop on every request, so a shortcut changed in its
settings shows here at once.

### Through the portal

The portal keeps a shortcut only while the daemon holds the session it is bound
in. So the daemon keeps the ones the user added in its config, and binds them
again at every start. It registers as the settings app, `super-voice-app`, so
the desktop names Super Voice in its dialog and its shortcut settings.

1. The desktop may ask the user before it binds one, in a dialog of its own,
   and may put it on other keys than it asked for. `keys` are the desktop's
   description of the keys it is on, like `Meta+Space`, and empty when it does
   not say.
2. On Hyprland nothing is bound until the user binds `super-voice-app:dictate`
   to keys in its config, with `global`.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `GET /shortcuts`

```http
GET /shortcuts HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
```

**Response (200):**

```json
{
  "status": "success",
  "method": "cosmic",
  "shortcuts": [
    { "id": "dictate", "state": "installed", "keys": "Super+Space", "command": "voice record --write" }
  ]
}
```

## `POST /shortcuts/{id}`

Add the shortcut on its own keys. Adding one that is set up already changes
nothing and succeeds. No request body. Answers the whole list, as
`GET /shortcuts` does.

It is never added on keys another shortcut holds. On COSMIC, the entry runs the
`voice` installed beside the daemon.

Through the portal, the request waits on the desktop's answer, up to 60 seconds,
since the desktop may ask the user first.

**Errors:**

| HTTP | `error_code`            | Meaning                                                                 |
|------|-------------------------|-------------------------------------------------------------------------|
| 404  | `not_found`             | No shortcut has that id (`unknown_shortcut`)                            |
| 409  | `shortcut_keys_taken`   | Another shortcut holds its keys. Bind `command` to other keys instead.  |
| 409  | `shortcuts_unsupported` | The `manual` method: bind `command` in the desktop's settings instead.  |
| 409  | `shortcut_refused`      | The user said no in the desktop's dialog. Asking again asks again.      |
| 500  | `internal`              | The desktop's settings could not be changed                             |

## `DELETE /shortcuts/{id}`

Remove the shortcut, from whatever keys it is on. Removing one that is not set
up changes nothing and succeeds. Answers the whole list.

On COSMIC, removing `dictate` hands Super+Space back to COSMIC's own binding for
it, switching the input source.

**Errors:**

| HTTP | `error_code` | Meaning                                       |
|------|--------------|-----------------------------------------------|
| 404  | `not_found`  | No shortcut has that id (`unknown_shortcut`)  |
| 500  | `internal`   | The desktop's settings could not be changed   |

## Errors on every route

| HTTP | `message`         | Meaning                                  |
|------|-------------------|------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |
| 429  | `rate_limited`    | Too many requests; back off              |
