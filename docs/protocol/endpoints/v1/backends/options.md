# `/backends/{source}/options`

Read, set, and reset a backend's **options** — the non-sensitive
configuration values (a base-URL override, a timeout, and so on) a backend
declares as `[[options]]` in its
[`backend.toml`](../../../backend/config.md). The daemon stores option
overrides as plaintext in its config and injects each as an
`x-stt-option-<name>` request header at model-load time (see
[contract.md](../../../backend/contract.md#request-headers)).

`{source}` is the backend's repo id (e.g. `github.com/super-stt/openai`),
**URL-percent-encoded** in the path — the same identifier used by
[`DELETE /backends/{source}`](../backends.md#delete-backendssource):

```
/backends/github.com%2Fsuper-stt%2Fopenai/options/base_url
```

These endpoints mirror the [secrets](./secrets.md) endpoints exactly, with two
differences: options are gated by the `settings` scope (not `secrets`), and —
because option values are not sensitive — a `GET` **returns the value**.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required on every request.
- A token without the `settings` scope gets `403 scope_denied`.

## Declared-option guard

`{name}` must be an option the backend **declares** in its `[[options]]`. An
unknown `{name}` returns `404 unknown_option`; an unknown `{source}` returns
`404 unknown_backend`. `list` is reserved for the collection endpoint, so a
backend cannot declare an option named `list`.

## Effective value vs. default

Each option has a manifest **default** and an optional user **override**. The
*effective value* is the override when set, otherwise the default. `POST` sets
the override; `DELETE` removes it, resetting the effective value back to the
default.

## `GET /backends/{source}/options/list`

List the backend's declared options with their effective values.

**Request:**

```http
GET /backends/github.com%2Fsuper-stt%2Fopenai/options/list HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{
  "status": "success",
  "options": [
    {
      "name":     "base_url",
      "label":    "API base URL",
      "type":     "string",
      "default":  "https://api.openai.com",
      "required": false,
      "value":    "https://api.openai.com"  // effective value (override or default)
    }
  ]
}
```

| Field          | Type             | Notes                                                          |
|----------------|------------------|----------------------------------------------------------------|
| `options`      | array of objects | One per declared option.                                       |
| `…[].name`     | string           | The declared option `name` (snake_case).                       |
| `…[].label`    | string           | Human-readable label; falls back to `name` when absent.        |
| `…[].type`     | string           | Declared value type (e.g. `string`).                           |
| `…[].default`  | any              | Manifest default; the effective value when no override is set. |
| `…[].required` | boolean          | Whether the backend needs it to operate.                      |
| `…[].value`    | any              | Effective value: the override if set, else `default`.          |

## `GET /backends/{source}/options/{name}`

Read one option's effective value.

**Request:**

```http
GET /backends/github.com%2Fsuper-stt%2Fopenai/options/base_url HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{
  "status":  "success",
  "name":    "base_url",
  "value":   "https://gateway.example.com",  // effective value
  "default": "https://api.openai.com"
}
```

## `POST /backends/{source}/options/{name}`

Set the option override. Takes effect the next time that backend's model is
loaded.

**Request:**

```http
POST /backends/github.com%2Fsuper-stt%2Fopenai/options/base_url HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{ "value": "https://gateway.example.com" }
```

| Field   | Type   | Required | Notes                                                  |
|---------|--------|----------|--------------------------------------------------------|
| `value` | string | yes      | New override value. Use `DELETE` to reset to default.  |

**Response (200):**

```jsonc
{ "status": "success", "value": "https://gateway.example.com" }
```

## `DELETE /backends/{source}/options/{name}`

Remove the override, resetting the option to its manifest **default**.
Idempotent: resetting an option that has no override succeeds. The returned
`value` is the effective value after the reset — i.e. the default.

**Request:**

```http
DELETE /backends/github.com%2Fsuper-stt%2Fopenai/options/base_url HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{ "status": "success", "value": "https://api.openai.com" }
```

## Errors

| HTTP | `message`         | Meaning                                              |
|------|-------------------|------------------------------------------------------|
| 400  | `invalid_request` | Malformed body.                                      |
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`.             |
| 403  | `scope_denied`    | Token lacks the `settings` scope.                    |
| 404  | `unknown_backend` | No installed backend has that `source`.             |
| 404  | `unknown_option`  | `{name}` is not a declared option of that backend.  |
