# `GET /pipeline/{stage}/backend/list`

The installed backends that can fill one [stage](./stage.md): those serving at
least one model carrying its role.

The slot itself is [`/pipeline/{stage}`](./stage.md) one level up — `GET`
reports the backend filling the position, `POST` chooses it. This is the menu
that `POST` accepts, the same relationship [`/model/list`](./model-list.md) has
with [`/model`](./model.md) and
[`/device/list`](./device.md#get-pipelinestagemodelmodeldevicelist) with
[`/device`](./device.md#get-pipelinestagemodelmodeldevice).

**Fill a stage's backend picker from this, not from
[`GET /backend/list`](../backend/list.md).** A backend serving nothing this stage can
run is refused by `POST /pipeline/{stage}`, so offering one hands the user an
error to discover by choosing it. The daemon already applies this rule when it
accepts or rejects a selection; a client filtering on its own is reimplementing
it, and the two can drift.

A backend serving **both** roles appears in both stages' lists — with only its
own stage's models, which is [`/model/list`](./model-list.md)'s business.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- A token without the `settings` scope gets `403 scope_denied`.

## Request

```http
GET /pipeline/1/backend/list HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

## Response (200)

The same objects [`GET /backend/list`](../backend/list.md) returns, narrowed to this
stage:

```jsonc
{
  "status": "success",
  "backends": [
    {
      "source":  "github.com/super-stt/whisper",
      "name":    "Whisper (local)",
      "version": "1.2.0",
      "kind":    "wasm",
      "models":  [ /* … */ ],
      "secrets": [ /* … */ ],
      "options": [ /* … */ ]
    }
  ]
}
```

**Empty when nothing installed serves this stage.** That is a real state, not an
error: it should read as "install one" rather than as an empty dropdown. The
Library — [`GET /registry/backend/list`](../registry.md) — is where one comes from.

## Errors

| HTTP | `error_code`      | Meaning                                 |
|------|-------------------|------------------------------------------|
| 404  | `unknown_stage`   | No such position in the pipeline         |
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |
