# `GET /auth/status`

Probe whether the held bearer token is still valid, without
triggering a consent popup. Useful for headless / CLI clients that
want to fail-fast on startup instead of synchronously blocking on a
GUI prompt the host can't display.

A positive response does **not** extend the token's expiry. A
negative response does **not** trigger consent.

## Auth

- **Required scope:** any authenticated (any valid token, regardless of scopes).
- `Authorization: Bearer <session_token>` is required.

## `GET /auth/status`

**Request:**

```http
GET /auth/status HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200, valid token):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":     "success",
  "scopes":     ["settings", "status", "daemon_status"],
  "expires_at": "2026-06-04T12:34:56Z"
}
```

| Field        | Type     | Notes                                                          |
|--------------|----------|----------------------------------------------------------------|
| `scopes`     | string[] | The scope set the token was issued under                       |
| `expires_at` | string   | ISO 8601 UTC timestamp the token expires                       |

**Response (401, invalid token):**

```http
HTTP/1.1 401 Unauthorized
Content-Type: application/json

{
  "status":  "error",
  "message": "invalid_session",
  "data":    { "reason": "expired" }
}
```

| `data.reason`  | Why                                                                                              | Client should                                                |
|----------------|--------------------------------------------------------------------------------------------------|--------------------------------------------------------------|
| `unknown`      | Token isn't recognized — never issued, or revoked since.                                         | Drop the cached token; call [`POST /auth/request`](./request.md).      |
| `expired`      | Token is older than its `expires_at` (30 days from issue).                                       | Same — drop and re-auth.                                     |
| `exe_changed`  | The client's binary path no longer matches what the user approved.                               | Same — the user must consent again to the new binary.        |
