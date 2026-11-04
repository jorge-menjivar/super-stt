# `/backend/{backend_id}/option/list`

Read, set, and reset a backend's **options** — the non-sensitive
configuration values (a base-URL override, a speaking-style preset, a timeout)
a backend declares as `[[options]]` in its
[`backend.toml`](../../../backend/config.md). The daemon stores option
overrides as plaintext in its config and injects each as an
`x-tts-option-<name>` request header on every `/v1` request (see
[contract.md](../../../backend/contract.md#request-headers)).

A write takes effect on the backend's **next request**, not its next model
load: the daemon hands the running instance the new value rather than reloading
it. Setting `base_url` is no exception — the endpoint it authorizes is checked
per outbound connection, so that too is swapped in place.

`{backend_id}` is the backend's id — its `source` as
[`GET /backend/list`](../backends.md) reports it (e.g.
`github.com/super-tts/openai`), **URL-percent-encoded** in the path — the same
identifier used by
[`DELETE /backend/{backend_id}`](../backends.md#delete-backendbackend_id):

```
/backend/github.com%2Fsuper-tts%2Fopenai/option/base_url
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
unknown `{name}` returns `404 unknown_option`; an unknown `{backend_id}`
returns `404 unknown_backend`. `list` is reserved for the collection endpoint,
so a backend cannot declare an option named `list`.

## Effective value vs. default

Each option has a manifest **default** and an optional user **override**. The
*effective value* is the override when set, otherwise the default. `POST` sets
the override; `DELETE` removes it, resetting the effective value back to the
default.

## Open-ended options and closed sets

An option is either free-form or drawn from a fixed list, and `choices` is how
the manifest says which. An empty `choices` is open-ended — `base_url` takes
any endpoint the operator can name — and a client renders it as a text field.
A non-empty `choices` is a closed set: a dropdown, and the daemon refuses a
`POST` of anything outside it with `400 invalid_value`.

The refusal matters because the settings UI is not the only thing that writes
here. Before `choices` existed, an option whose accepted values were listed in
its help text rendered as a free-text box: typing `formalish` into a styling
option was accepted, stored, and injected into the load headers as though it
were a value the backend knew — and the failure surfaced, if at all, as
strange-sounding speech rather than as an error. A stored value has to stay one
the backend understands, so the daemon validates it rather than trusting the
picker.

A dropdown behaves like a switch rather than like a text field: picking is the
write, and picking the declared default clears the override instead of storing
a copy of it — so a backend that later changes its default takes effect for
users who never chose otherwise. A stored value the backend has since dropped
from its list selects nothing rather than the nearest wrong row, and the user
picks again.

## `GET /backend/{backend_id}/option/list`

List the backend's declared options with their effective values.

**Request:**

```http
GET /backend/github.com%2Fsuper-tts%2Fopenai/option/list HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
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
      "choices":  [],                       // open-ended; a text field
      "value":    "https://api.openai.com"  // effective value (override or default)
    },
    {
      "name":     "styling",
      "label":    "Styling",
      "type":     "string",
      "default":  "semi-formal",
      "required": false,
      "choices":  ["casual", "semi-formal", "formal"],  // closed set; a dropdown
      "value":    "formal"
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
| `…[].choices`  | array            | The values this option accepts. Empty means any value of `type`, which a client renders as a text field; a non-empty list is a dropdown, and a `POST` of anything outside it is refused. |
| `…[].required` | boolean          | Whether the backend needs it to operate.                      |
| `…[].value`    | any              | Effective value: the override if set, else `default`.          |

A `bool` option never declares `choices` — a switch already names its two
values — and a manifest that puts one there is rejected before it is
published, along with one whose `default` sits outside its own list or whose
list repeats a value.

## `GET /backend/{backend_id}/option/{name}`

Read one option's effective value.

**Request:**

```http
GET /backend/github.com%2Fsuper-tts%2Fopenai/option/base_url HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
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

## `POST /backend/{backend_id}/option/{name}`

Set the option override. A model currently running from that backend is handed
the new value immediately and uses it from its next request; nothing is
reloaded. A backend that is not loaded picks the value up when it loads.

Writing the value already stored is a no-op, reported as such in `message`.

If the new value cannot be delivered to the running model — its backend is no
longer installed, or a required secret has gone missing — the write still
succeeds, because the override is stored either way, and `message` says the
running backend kept the old value. It is never left with nothing loaded.

`base_url` is validated here rather than at the next load: a value no host can
be read from is refused with `400 invalid_value`, and nothing is stored.

Every option value is checked against what a request header can carry, because
that is how it is delivered: the daemon injects it as `x-tts-option-<name>` on
every `/v1` call. A value longer than 4000 characters, or carrying a control
character, is refused with `400 invalid_value` and nothing is stored. Refusing
the write is the only point at which the user learns — a stored value that
cannot be delivered reports success, shows in the settings UI, and then fails
every request the backend makes with an error naming a header rather than the
setting they typed.

**Request:**

```http
POST /backend/github.com%2Fsuper-tts%2Fopenai/option/base_url HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
Content-Type: application/json

{ "value": "https://gateway.example.com" }
```

| Field   | Type   | Required | Notes                                                  |
|---------|--------|----------|--------------------------------------------------------|
| `value` | string | yes      | New override value. Use `DELETE` to reset to default. When the option declares `choices`, must be one of them. |

**Response (200):**

```jsonc
{ "status": "success", "value": "https://gateway.example.com" }
```

`base_url` is the one option stored in a rewritten form: the value is
canonicalized to `scheme://host[:port][/path]` — the same form the backend is
handed at model load — so the `value` returned here, and the one a later `GET`
reports, is the endpoint that will actually be dialed rather than the string
that was posted. A value naming no scheme is given the one its host implies, so
posting `192.168.0.179:8080/v1` stores and returns
`http://192.168.0.179:8080/v1`. See
[config.md](../../../backend/config.md#base_url-and-egress) for the full rule.
Every other option is stored exactly as posted.

## `DELETE /backend/{backend_id}/option/{name}`

Remove the override, resetting the option to its manifest **default**.
Idempotent: resetting an option that has no override succeeds. The returned
`value` is the effective value after the reset — i.e. the default.

**Request:**

```http
DELETE /backend/github.com%2Fsuper-tts%2Fopenai/option/base_url HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
```

**Response (200):**

```jsonc
{ "status": "success", "value": "https://api.openai.com" }
```

## Errors

| HTTP | `message`         | Meaning                                              |
|------|-------------------|------------------------------------------------------|
| 400  | `invalid_request` | Malformed body, or an empty `value`.                 |
| 400  | `invalid_value`   | The option declares `choices` and the posted value is not one of them (the `message` names the values on offer); or the value is one a request header cannot carry — over 4000 characters, or containing a control character. |
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`.             |
| 403  | `scope_denied`    | Token lacks the `settings` scope.                    |
| 404  | `unknown_backend` | No installed backend has that `source`.              |
| 404  | `unknown_option`  | `{name}` is not a declared option of that backend.   |
