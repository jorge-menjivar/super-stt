# `GET /pipeline/{stage}/model/list`

The models a [stage](./stage.md) can run: the ones its backend serves **in that
stage's role**. This is what fills a model picker.

Each entry is a `[name, source]` pair, where `source` is the repo id of the
backend serving it (see [`docs/protocol/backend/`](../../../backend/)). That is
the pair [`POST /pipeline/{stage}/model`](./model.md#post-pipelinestagemodel)
accepts.

## Scoped twice, and both halves matter

**To the stage's backend.** Only the backend
[filling this stage](./stage.md#post-pipelinestage) is consulted. A model from
another backend cannot load here without also changing the stage's backend, so
offering it in this list hands the user a pick that does something other than
what the list implied. Empty when the stage has no backend selected — which is
the daemon's idle state, and reads correctly as "choose a backend first".

**To the stage's role.** Stage 1 lists `synthesis` models. Today that is every
model a TTS backend serves, so the narrowing is invisible; it stops being
invisible the moment a second position exists and a backend serves models for
both. Filtering in the daemon rather than in each client is what keeps the two
from drifting: the daemon is the side that decides whether a load is accepted,
so it should be the side that decides what is offered.

The full catalog — every installed backend and every model it serves,
with the voices, language tags and device support of each — is
[`GET /backend/list`](../backends.md). This endpoint is the narrow per-stage
read, answered by the daemon precisely so a client does not have to re-derive
roles for itself.

> **Replaces `GET /models`,** which read the one active backend and had no way
> to express a position. Addressing the list by position means a second stage
> needs no second endpoint.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `GET /pipeline/{stage}/model/list`

| Param   | Type | Notes                                                                       |
|---------|------|-----------------------------------------------------------------------------|
| `stage` | int  | Pipeline position. `1` synthesizes. A position that does not exist is a `404 unknown_stage`. |

**Request:**

```http
GET /pipeline/1/model/list HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
```

**Response (200):**

```jsonc
{
  "status": "success",
  "available_models": [
    ["kokoro-82m", "github.com/jorge-menjivar/super-tts-kokoro"],
    ["qwen3-tts-1.7b-base", "github.com/super-libre/super-tts-qwen"]
  ]
}
```

| Field              | Type            | Notes                                                                 |
|--------------------|-----------------|-----------------------------------------------------------------------|
| `available_models` | array of arrays | Each entry is the `[name, source]` pair `POST /pipeline/{stage}/model` accepts. Empty when the stage has no backend, or when its backend serves nothing in that role. |

Names only. A picker that wants to show what each model can do — its preset
voices, whether it clones, its `max_input_chars` — joins these names against
[`GET /backend/list`](../backends.md), which carries the manifest entry for
every installed model. Repeating that here would make the picker's cheapest
call its most expensive one.

**Errors:**

| HTTP | `error_code`      | Meaning                                 |
|------|-------------------|-----------------------------------------|
| 404  | `unknown_stage`   | No such position in the pipeline        |
| 401  | `invalid_session` | Token unknown / expired / `exe_changed` |
| 403  | `scope_denied`    | Token lacks the `settings` scope        |
