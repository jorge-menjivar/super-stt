# `/active_backend`

Read, select, and clear the **active backend** — the installed provider the
daemon is currently set to use. This is distinct from the
[active model](./active_model.md): selecting a backend records *which provider*
is active and validates its installed files, but does **not** load a model, so
it cannot fail for runtime reasons (a missing API key, a download). Loading a
model — the step that can fail that way — happens through
[`POST /active_model`](./active_model.md).

A backend is identified on the wire by its `source` (repo id, e.g.
`github.com/super-stt/mistral`), as returned by [`GET /backends`](./backends.md).
Internally the daemon persists the backend's install directory and re-reads its
`backend.toml` for metadata, so the selection survives a reinstall.

Selecting a backend that is *not* the one already loaded unloads the current
model: the daemon is then "backend selected, no model loaded" until a model is
chosen. At startup such a state comes up idle (no model is auto-loaded).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `GET /active_backend`

**Request:**

```http
GET /active_backend HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{
  "status": "success",
  // null when no backend is selected (daemon idle).
  "active_backend": {
    "source":       "github.com/super-stt/mistral",
    "name":         "Mistral",
    "model_loaded": false   // whether a model from this backend is loaded
  }
}
```

| Field                        | Type    | Notes                                                       |
|------------------------------|---------|-------------------------------------------------------------|
| `active_backend`             | object? | `null` when idle (no backend selected)                      |
| `active_backend.source`      | string  | Repo id of the selected backend                             |
| `active_backend.name`        | string  | Human-readable name (from its `backend.toml`)               |
| `active_backend.model_loaded`| bool    | Whether a model served by this backend is currently loaded  |

**Errors:**

| HTTP | `message`         | Meaning                                  |
|------|-------------------|------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |

## `POST /active_backend`

Select the active backend. Returns `200 OK`. Never fails for runtime reasons —
only if the backend's installed files are missing or invalid.

**Request:**

```http
POST /active_backend HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "source": "github.com/super-stt/mistral"
}
```

| Field    | Type   | Required | Notes                                                |
|----------|--------|----------|------------------------------------------------------|
| `source` | string | yes      | Repo id of an installed backend (from `GET /backends`) |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":         "success",
  "active_backend": { "source": "github.com/super-stt/mistral", "name": "Mistral", "model_loaded": false }
}
```

**Errors:**

| HTTP | `message`                | Meaning                                                              |
|------|--------------------------|----------------------------------------------------------------------|
| 400  | `invalid_backend`        | No installed backend with that `source`, or its files are missing/invalid |
| 401  | `invalid_session`        | Token unknown / expired / `exe_changed`                              |
| 403  | `scope_denied`           | Token lacks the `settings` scope                                     |
| 409  | `recording_in_progress`  | A recording or real-time session is active; stop it first            |

## `DELETE /active_backend`

Deselect the active backend: unload any loaded model and return the daemon to
idle.

**Request:**

```http
DELETE /active_backend HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "success"
}
```

**Errors:**

| HTTP | `message`                | Meaning                                                   |
|------|--------------------------|-----------------------------------------------------------|
| 401  | `invalid_session`        | Token unknown / expired / `exe_changed`                   |
| 403  | `scope_denied`           | Token lacks the `settings` scope                          |
| 409  | `recording_in_progress`  | A recording or real-time session is active; stop it first |
