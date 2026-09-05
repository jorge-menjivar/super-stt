# `/pipeline/{stage}`

One **stage** of the [pipeline](../pipeline.md), addressed by position: which
backend fills it, and whether it is filled at all. Every stage answers these
three verbs, so a client learns one shape and applies it anywhere.

**Only the backend.** What that backend is running is
[`/pipeline/{stage}/model`](./model.md), one level down, and filling a stage is
not loading a model. The split is deliberate: choosing a backend is cheap and
cannot fail for runtime reasons, while loading a model downloads, allocates and
can fail — and a backend selection outlives every model that comes and goes
under it.

The [stage object](../pipeline.md#the-stage-object) these endpoints report, the
stage roles and the [auth](../pipeline.md#auth) they all share are described
once in the family overview.

## `GET /pipeline/{stage}`

One stage, as `stage` — the same object the array carries.

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "success",
  "stage": {
    "stage":   2,
    "role":    "post_processor",
    "source":  "github.com/jorge-menjivar/super-stt-textclean",
    "name":    "Text Cleanup",
    "enabled": true
  }
}
```

The model is not here — read it at
[`GET /pipeline/{stage}/model`](./model.md#get-pipelinestagemodel).

## `POST /pipeline/{stage}`

Select the backend that fills this stage. Validates that it is installed and
serves this stage's role; does **not** load anything.

The backends it will accept are
[`GET /pipeline/{stage}/backend/list`](./backend-list.md) — fill a picker from
that rather than from `GET /backends`, or it offers backends this refuses.

**Request:**

```http
POST /pipeline/2 HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{ "source": "github.com/jorge-menjivar/super-stt-textclean" }
```

| Field    | Type   | Required | Notes                                              |
|----------|--------|----------|----------------------------------------------------|
| `source` | string | yes      | Repo id of an installed backend serving this role  |

Selecting a **different** backend drops the model with it — the name belonged to
the old backend — and switches the stage off until a model is chosen.
Re-selecting the backend already there changes nothing.

**Errors:**

| HTTP | `error_code`            | Meaning                                                          |
|------|-------------------------|------------------------------------------------------------------|
| 404  | `unknown_stage`         | No such position in the pipeline                                 |
| 400  | `invalid_value`         | `source` missing                                                 |
| 400  | `invalid_backend`       | No installed backend with that `source` serves this stage's role |
| 409  | `recording_in_progress` | A recording is in flight                                         |
| 401  | `invalid_session`       | Token unknown / expired / `exe_changed`                          |
| 403  | `scope_denied`          | Token lacks the `settings` scope                                 |

## `DELETE /pipeline/{stage}`

Empty the stage: unload, and forget the model with the backend.

Stage 1 returns the daemon to idle. Stage 2 turns post-processing off entirely.

**Errors:** `404 unknown_stage`, `409 recording_in_progress`, plus the auth
errors above.
