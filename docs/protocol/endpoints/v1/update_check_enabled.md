# `/update_check_enabled`

Whether the daemon periodically checks for available updates and
sends a desktop notification when one is found. Defaults to `true`.

Turning this off stops both the periodic check and its notification.
[`POST /update/check`](./update/check.md) still runs an on-demand
check regardless of this setting, and the last known status is
always available via [`GET /update`](./update.md).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /update_check_enabled`

**Request:**

```http
POST /update_check_enabled HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "enabled": false
}
```

| Field     | Type | Required | Notes                                                                     |
|-----------|------|----------|-----------------------------------------------------------------------------|
| `enabled` | bool | yes      | `true` runs the periodic check and its notification; `false` stops both. |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":               "success",
  "update_check_enabled": false
}
```

**Errors:**

| HTTP | `message`                     | Meaning                                    |
|------|--------------------------------|-----------------------------------------------|
| 400  | `invalid_update_check_enabled` | `enabled` missing or not a bool               |
| 401  | `invalid_session`              | Token unknown / expired / `exe_changed`       |
| 403  | `scope_denied`                 | Token lacks the `settings` scope              |

## `GET /update_check_enabled`

**Request:**

```http
GET /update_check_enabled HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":               "success",
  "update_check_enabled": true
}
```

**Errors:**

| HTTP | `message`         | Meaning                                  |
|------|-------------------|--------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |
