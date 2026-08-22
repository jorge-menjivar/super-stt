# `/update_beta_optin`

Read and set whether the daemon considers prerelease versions when
selecting the self-update candidate.

| Value      | Notes                                                                                           |
|------------|---------------------------------------------------------------------------------------------------|
| `auto`     | Opt in to prereleases iff the daemon's own running version is itself a prerelease (the default). |
| `enabled`  | Always consider prereleases when selecting the update candidate.                                  |
| `disabled` | Never consider prereleases; only stable releases are candidates.                                  |

The resolved value — always `true` or `false`, never `auto` — is
reported as `beta_optin_effective` on [`GET /update`](./update.md) and
[`POST /update/check`](./update/check.md).

A value stored on disk that fails to parse degrades to `auto` when
the daemon loads its config, the same per-field fallback every other
setting gets. A `POST` that supplies an invalid value is always
rejected with no state change — the fallback applies only to config
load, never to a wire `SET`.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /update_beta_optin`

**Request:**

```http
POST /update_beta_optin HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "value": "enabled"
}
```

| Field   | Type   | Required | Notes                                 |
|---------|--------|----------|------------------------------------------|
| `value` | string | yes      | One of `auto`, `enabled`, `disabled`  |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":             "success",
  "update_beta_optin":  "enabled"
}
```

**Errors:**

| HTTP | `message`                    | Meaning                                                                |
|------|--------------------------------|----------------------------------------------------------------------------|
| 400  | `invalid_update_beta_optin`    | `value` wasn't one of `auto`, `enabled`, `disabled` — no state change |
| 401  | `invalid_session`              | Token unknown / expired / `exe_changed`                                |
| 403  | `scope_denied`                 | Token lacks the `settings` scope                                       |

## `GET /update_beta_optin`

**Request:**

```http
GET /update_beta_optin HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":             "success",
  "update_beta_optin":  "auto"
}
```

**Errors:**

| HTTP | `message`         | Meaning                                  |
|------|-------------------|--------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |
