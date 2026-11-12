# `/shortcuts`

Super Voice's keyboard shortcuts on this desktop: where each one stands, adding
one, and removing one. The daemon owns them, so every client sees the same
answer, and a client that cannot write desktop settings, like a sandboxed app,
can still add one.

| `id`         | Does                                   | Its keys          | What it runs           |
|--------------|----------------------------------------|-------------------|------------------------|
| `dictate`    | Record, transcribe, and type the text  | Super+Space       | `voice record --write` |
| `read_aloud` | Read the selected or copied text aloud | Super+Shift+Space | `voice speak --read`   |

How the daemon puts a shortcut on the desktop is the `method`:

| `method`  | Desktop                                     | What the daemon does                                                                                   |
|-----------|---------------------------------------------|--------------------------------------------------------------------------------------------------------|
| `cosmic`  | COSMIC                                      | Writes an entry to COSMIC's custom shortcuts, which runs the command.                                 |
| `portal`  | One with the `GlobalShortcuts` portal, such as KDE Plasma, GNOME or Hyprland | Binds it in a portal session it holds, and runs the command when it is pressed. |
| `manual`  | Any other                                   | Nothing. The user binds `command` in the desktop's own keyboard settings.                             |

`removable` says whether `DELETE /shortcuts/{id}` takes a shortcut off the
desktop, so a client offers to remove one. It is true on COSMIC, and through the
portal on KDE Plasma. Through the portal elsewhere it is false: see below.

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
in. So the daemon keeps the ones the user added in its config, by desktop
(`XDG_CURRENT_DESKTOP`), and binds them again at every start on that desktop.
One added on another desktop is left for that one: binding it would have this
desktop ask the user about it, unasked. It registers as the settings app, `super-voice-app`, so
the desktop names Super Voice in its dialog and its shortcut settings.

1. The desktop may ask the user before it binds one, in a dialog of its own,
   and may put it on other keys than it asked for. `keys` are the desktop's
   description of the keys it is on, like `Meta+Space`, and empty when it does
   not say. KDE Plasma takes its dialog's Cancel as binding the shortcut with
   no keys, so on KDE one bound with none is `shortcut_refused`, and KDE is
   told to forget it.
2. On Hyprland nothing is bound until the user binds
   `super-voice-app:dictate` or `super-voice-app:read_aloud` to keys in its
   config, with `global`.
3. The portal cannot unbind a shortcut, and the desktop keeps one in its own
   settings, keys and all, until the user removes it there.
   1. On KDE Plasma, `DELETE` also tells KDE's shortcut service to forget it,
      so it leaves KDE's settings and frees its keys. KDE's own settings
      cannot remove a shortcut while the daemon holds it: they hide it until
      they reload, and it keeps working.
   2. Elsewhere a shortcut is changed or removed in the desktop's settings,
      and `removable` is false. `DELETE` only stops Super Voice binding it.
4. At each start the daemon binds the saved shortcuts again, and forgets one
   the user removed in the desktop's settings, so `GET /shortcuts` lists it
   as `missing` again:
   1. KDE Plasma says which it kept before they are bound, so only those are
      bound.
   2. GNOME lists nothing until a bind, so every saved one is bound, and
      GNOME asks again in its dialog for one removed in its settings. One the
      user turns down there is forgotten.
   3. Hyprland keeps nothing between sessions, so every saved one is bound.
5. GNOME switches the input source on Super+Space, and back on
   Super+Shift+Space, and its own binding gets a press before Super Voice's.
   So, as Dictate's entry does on COSMIC, the daemon takes a shortcut's keys
   from GNOME's input-source switching before it binds the shortcut, at each
   add and each start, and gives them back once it no longer binds it.
   Input-source switching keeps its other keys, such as the keyboard's own
   switch key. Only keys the daemon took are given back: ones the user freed
   stay free.

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
  "removable": true,
  "shortcuts": [
    { "id": "dictate",    "state": "installed", "keys": "Super+Space",       "command": "voice record --write" },
    { "id": "read_aloud", "state": "missing",   "keys": "Super+Shift+Space", "command": "voice speak --read" }
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
it, switching the input source. Through the portal on KDE Plasma, KDE's shortcut
service forgets it. Through the portal elsewhere, it only stops Super Voice
binding it: the desktop keeps it until the user removes it in its own settings.

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
