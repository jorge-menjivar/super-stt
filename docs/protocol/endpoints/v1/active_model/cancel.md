# `POST /active_model/cancel`

Abort an in-flight model switch — typically a model download. Once
the switch has passed the cancellable window (the new model is
already being loaded or has loaded successfully), this returns a
`409`.

The active model state itself is read and written via
[`/active_model`](../active_model.md).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- `client` / `widget` tokens get `403 scope_denied`.

## `POST /active_model/cancel`

**Request:**

```http
POST /active_model/cancel HTTP/1.1
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
  "message": "Model switch cancelled"
}
```

After a successful cancel the next
[`GET /active_model`](../active_model.md#get-active_model) reflects
the previous model as `current` and the cancelled download as
`switch: { phase: "cancelled", ... }` (cleared on the next switch
attempt).

**Errors:**

| HTTP | `message`               | Meaning                                                              |
|------|-------------------------|----------------------------------------------------------------------|
| 401  | `invalid_session`       | Token unknown / expired / `exe_changed`                              |
| 403  | `scope_denied`          | Not a `settings` token                                               |
| 409  | `no_switch_in_progress` | Nothing to cancel                                                    |
| 409  | `switch_finalizing`     | The switch has already passed the cancellable window — wait or kick off a new switch instead |
