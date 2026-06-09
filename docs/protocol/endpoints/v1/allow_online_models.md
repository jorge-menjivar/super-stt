# `/allow_online_models`

The privacy gate for online STT providers (OpenAI, Mistral,
Deepgram). While this flag is `false`, attempts to switch to an
online model via [`POST /active_model`](./active_model.md) are
rejected with `400 online_models_disabled`. Flipping `true` →
`false` while an online model is *currently* active reverts to a
local default; subscribers to
[`/events?topics=daemon_status_changed`](./events.md) see the
follow-up `status: "ready"` event for the new local model.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /allow_online_models`

**Request:**

```http
POST /allow_online_models HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "enabled": false
}
```

| Field     | Type | Required | Notes                                                                 |
|-----------|------|----------|-----------------------------------------------------------------------|
| `enabled` | bool | yes      | `true` allows online providers to be loaded; `false` blocks them      |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":               "success",
  "allow_online_models":  false,
  "message":              "Online models disabled — all transcription is local"
}
```

If an online model was active when the flag flipped `true` →
`false`, the active model is changed to a local default and
subscribers see `daemon_status_changed` with the new model name on
`/events`.

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Token lacks the `settings` scope                              |

## `GET /allow_online_models`

**Request:**

```http
GET /allow_online_models HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":              "success",
  "allow_online_models": true
}
```

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Token lacks the `settings` scope                              |
