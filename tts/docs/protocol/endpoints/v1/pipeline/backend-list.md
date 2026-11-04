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
[`GET /backend/list`](../backends.md).** A backend serving nothing this stage
can run is refused by [`POST /pipeline/{stage}`](./stage.md#post-pipelinestage),
so offering one hands the user an error to discover by choosing it. The daemon
already applies this rule when it accepts or rejects a selection; a client
filtering on its own is reimplementing it, and the two can drift.

Today, with `synthesis` the only role, this list and `GET /backend/list` hold
the same backends. That is a coincidence of having one stage, not a reason to
call the other endpoint: a second position would change one of the two lists
and not the other, and a client built on the general one would not have to
notice.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- A token without the `settings` scope gets `403 scope_denied`.

## Request

```http
GET /pipeline/1/backend/list HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
```

## Response (200)

The same objects [`GET /backend/list`](../backends.md) returns, narrowed to
this stage:

```jsonc
{
  "status": "success",
  "backends": [
    {
      "source":  "github.com/jorge-menjivar/super-tts-kokoro",
      "name":    "Kokoro",
      "version": "1.2.0",
      "kind":    "subprocess",
      "models":  [ /* … */ ],
      "secrets": [ /* … */ ],
      "options": [ /* … */ ]
    }
  ]
}
```

Every field is documented on [`GET /backend/list`](../backends.md); this
endpoint changes which backends appear, never their shape.

**Empty when nothing installed serves this stage.** That is a real state, not
an error: a fresh install has no backends at all, and the answer should read as
"install one" rather than as an empty dropdown. The Library —
[`GET /registry/backend/list`](../registry/backends.md) — is where one comes
from.

## Errors

| HTTP | `error_code`      | Meaning                                 |
|------|-------------------|-----------------------------------------|
| 404  | `unknown_stage`   | No such position in the pipeline        |
| 401  | `invalid_session` | Token unknown / expired / `exe_changed` |
| 403  | `scope_denied`    | Token lacks the `settings` scope        |
