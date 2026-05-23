# `GET /ping`

Liveness probe. Confirms the HTTP listener is reachable and the
token presented is valid. No state introspection.

To probe whether the held *token* is still valid (without spawning
a popup on failure), prefer [`GET /auth/status`](./auth/status.md)
— that's the dedicated probe.

## Auth

- **Required scope:** any authenticated (`client`, `settings`, or
  `widget`).
- `Authorization: Bearer <session_token>` is required.

## `GET /ping`

**Request:**

```http
GET /ping HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":  "success",
  "message": "pong"
}
```

| Field     | Type   | Notes                                  |
|-----------|--------|----------------------------------------|
| `message` | string | Always `"pong"` on a healthy daemon    |

**Errors:**

| HTTP | `message`             | Meaning                                                       |
|------|-----------------------|---------------------------------------------------------------|
| 401  | `invalid_session`     | Token unknown / expired / `exe_changed` — re-auth and retry   |
| 429  | `rate_limited`        | Per-client rate limit hit; back off and retry                 |
| 503  | `connection_rejected` | Server refused the connection (overloaded)                    |
