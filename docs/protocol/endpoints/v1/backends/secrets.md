# `/backends/{source}/secrets`

Store, check, and clear a backend's **secrets** — the sensitive values
(API keys and the like) a backend declares as `[[secrets]]` in its
[`backend.toml`](../../../backend/config.md). The daemon owns secret storage
end to end: a client sets a secret here, the daemon persists it in the system
keyring, and the daemon reads it back **only** at model-load time to inject it
as an `x-stt-secret-<name>` request header (see
[contract.md](../../../backend/contract.md#request-headers)).

`{source}` is the backend's repo id (e.g. `github.com/super-stt/openai`),
**URL-percent-encoded** in the path — the same identifier used by
[`DELETE /backends/{source}`](../backends.md#delete-backendssource):

```
/backends/github.com%2Fsuper-stt%2Fopenai/secrets/openai_api_key
```

## Write-only by contract

Secret **values never leave the daemon over this API.** Clients can:

- **set** a value (`POST`),
- learn whether one is **configured** (`GET`), and
- **clear** one (`DELETE`),

but no endpoint ever returns a stored secret value. A `GET` reports only a
boolean `configured`. The sole reader of a secret's value is the daemon's own
model-load path; it is never serialized into an HTTP response. This is the
defining difference from [options](./options.md), whose non-sensitive values
*are* returned.

## Auth

- **Required scope:** `secrets`.
- `Authorization: Bearer <session_token>` is required on every request.
- A token without the `secrets` scope gets `403 scope_denied`. The `secrets`
  scope is independent of `settings` — managing a backend's options does not
  grant the ability to write its credentials, and vice versa.

## Declared-secret guard

`{name}` must be a secret the backend **declares**. The endpoint can only
read or write secrets that appear in the backend's `[[secrets]]`; it is not a
general-purpose keyring. A `{name}` that the backend does not declare returns
`404 unknown_secret`; an unknown `{source}` returns `404 unknown_backend`. A
*declared but unset* secret is not an error — it reports `configured: false`.

`list` is reserved for the collection endpoint below, so a backend cannot
declare a secret named `list`.

## `GET /backends/{source}/secrets/list`

List the backend's declared secrets and whether each is configured. **No
values.**

**Request:**

```http
GET /backends/github.com%2Fsuper-stt%2Fopenai/secrets/list HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{
  "status": "success",
  "secrets": [
    {
      "name":       "openai_api_key",
      "label":      "OpenAI API key",
      "required":   true,
      "configured": true        // whether a value is stored — never the value
    }
  ]
}
```

| Field            | Type             | Notes                                                             |
|------------------|------------------|-------------------------------------------------------------------|
| `secrets`        | array of objects | One per declared secret.                                          |
| `…[].name`       | string           | The declared secret `name` (snake_case).                          |
| `…[].label`      | string           | Human-readable label; falls back to `name` when absent.           |
| `…[].required`   | boolean          | Whether the backend needs it to operate.                         |
| `…[].configured` | boolean          | `true` when a value is stored. The value itself is never returned. |

## `GET /backends/{source}/secrets/{name}`

Report whether one secret is configured. **No value.**

**Request:**

```http
GET /backends/github.com%2Fsuper-stt%2Fopenai/secrets/openai_api_key HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{ "status": "success", "name": "openai_api_key", "configured": true }
```

`configured` is `false` for a declared-but-unset secret (still `200`, not an
error).

## `POST /backends/{source}/secrets/{name}`

Store (or replace) the secret's value. The value travels **only** in the
request body — never in the URL or query — so it does not land in logs or
shell history. The change takes effect the next time that backend's model is
loaded.

**Request:**

```http
POST /backends/github.com%2Fsuper-stt%2Fopenai/secrets/openai_api_key HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{ "value": "sk-…" }
```

| Field   | Type   | Required | Notes                                                         |
|---------|--------|----------|---------------------------------------------------------------|
| `value` | string | yes      | The secret value. Must be non-empty — use `DELETE` to clear. |

**Response (200):**

```jsonc
{ "status": "success", "configured": true }
```

## `DELETE /backends/{source}/secrets/{name}`

Clear the stored secret, resetting it to its default state — **unset**. A
secret has no default value, so clearing it simply removes the credential; the
backend then runs without it (and may fail to authenticate, the correct
"unconfigured" behavior). Idempotent: clearing an already-unset secret
succeeds.

**Request:**

```http
DELETE /backends/github.com%2Fsuper-stt%2Fopenai/secrets/openai_api_key HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{ "status": "success", "configured": false }
```

## Errors

| HTTP | `message`             | Meaning                                                        |
|------|-----------------------|----------------------------------------------------------------|
| 400  | `invalid_request`     | Malformed body, or an empty `value` on `POST`.                |
| 401  | `invalid_session`     | Token unknown / expired / `exe_changed`.                      |
| 403  | `scope_denied`        | Token lacks the `secrets` scope.                              |
| 404  | `unknown_backend`     | No installed backend has that `source`.                      |
| 404  | `unknown_secret`      | `{name}` is not a declared secret of that backend.           |
| 503  | `keyring_unavailable` | The system keyring could not be read or written.             |
