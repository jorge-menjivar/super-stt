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
another backend cannot load here, so offering it gives the user a pick that
fails. Empty when the stage has no backend selected.

**To the stage's role.** Stage 1 lists transcription models; stage 2 lists
post-processors. A backend may serve both, and the wrong list is worse than an
empty one: a post-processor selected as a transcription model loads
successfully and then fails on every recording — after the user has already
spoken.

The full catalog, every installed backend and every role, is
[`GET /backends`](../backends.md). This endpoint is the narrow per-stage read,
answered by the daemon precisely so a client does not have to re-derive roles
for itself.

> **Replaces `GET /models`,** which read stage 1's backend and filtered
> post-processors out. It could not express stage 2 at all, so clients derived
> that list themselves from `GET /backends`. Addressing the list by position
> means a third stage needs no third endpoint.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `GET /pipeline/{stage}/model/list`

| Param   | Type | Notes                                                                                  |
|---------|------|-----------------------------------------------------------------------------------------|
| `stage` | int  | Pipeline position: `1` transcribes, `2` post-processes. A position that does not exist is a `404 unknown_stage`. |

**Request:**

```http
GET /pipeline/1/model/list HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{
  "status": "success",
  "available_models": [
    ["voxtral-mini", "github.com/super-stt/voxtral"],
    ["whisper-1", "github.com/super-stt/openai"]
  ]
}
```

| Field              | Type            | Notes                                                                 |
|--------------------|-----------------|-----------------------------------------------------------------------|
| `available_models` | array of arrays | Each entry is the `[name, source]` pair `POST /pipeline/{stage}/model` accepts. Empty when the stage has no backend, or when its backend serves nothing in that role. |

**Errors:**

| HTTP | `error_code`      | Meaning                                  |
|------|-------------------|------------------------------------------|
| 404  | `unknown_stage`   | No such position in the pipeline         |
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |
