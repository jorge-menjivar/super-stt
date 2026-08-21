# `POST /update/check`

Force an immediate self-update check, bypassing the periodic
schedule. The response is the same status shape as
[`GET /update`](../update.md), refreshed by the check that just ran.

The check runs synchronously even when
[`update_check_enabled`](../update_check_enabled.md) is `false` —
this endpoint is the explicit, on-demand path and is never gated by
that setting. Concurrent calls serialize: a caller that arrives
while a check is already in flight waits for it to finish and
receives that same fresh result, rather than starting a second
check. A network failure during the check is reported in
`last_check_error`; the response is still `200`, never a `5xx`.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /update/check`

**Request:**

```http
POST /update/check HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{}
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "current_version": "0.2.2-beta.2",
  "latest_version": "v0.2.3-beta.1",
  "update_available": true,
  "checked_at": "2026-08-20T17:00:00Z",
  "last_check_error": null,
  "beta_optin_effective": true,
  "installer_asset": {
    "name": "super-stt-install-x86_64-unknown-linux-gnu",
    "url": "https://github.com/jorge-menjivar/super-stt/releases/download/v0.2.3-beta.1/super-stt-install-x86_64-unknown-linux-gnu",
    "size": 8388608,
    "sha256": "a3f2c8b1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1"
  }
}
```

See [`GET /update`](../update.md) for the field table.

**Errors:**

| HTTP | `message`         | Meaning                                  |
|------|-------------------|--------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |
