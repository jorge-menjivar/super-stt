# `/pipeline/{stage}`

One **stage** of the [pipeline](../pipeline.md), addressed by position: which
backend fills it, and whether it is filled at all. Every stage answers these
three verbs, so a client learns one shape and applies it anywhere.

**Only the backend.** What that backend is running is
[`/pipeline/{stage}/model`](./model.md), one level down, and filling a stage is
not loading a model. The split is deliberate: selecting a backend records
*which* backend is active and validates its installed files on disk, so it
cannot fail for runtime reasons — a missing API key, a download that stalls.
Loading a model is the step that can fail that way, and a backend selection
outlives every model that comes and goes under it.

A backend is identified on the wire by its `source` (repo id, e.g.
`github.com/jorge-menjivar/super-tts-kokoro`), as returned by
[`GET /backend/list`](../backends.md). Internally the daemon persists the
backend's install directory and re-reads its `backend.toml` for metadata, so
the selection survives a reinstall.

The [stage object](../pipeline.md#the-stage-object) these endpoints report, the
stage roles and the [auth](../pipeline.md#auth) they all share are described
once in the family overview.

## `GET /pipeline/{stage}`

One stage, as `stage` — the same object
[`GET /pipeline`](../pipeline.md#get-pipeline)'s array carries.

**Request:**

```http
GET /pipeline/1 HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "success",
  "stage": {
    "stage":   1,
    "role":    "synthesis",
    "source":  "github.com/jorge-menjivar/super-tts-kokoro",
    "name":    "Kokoro",
    "enabled": true
  }
}
```

An empty stage reports `source: null`, `name: null`, `enabled: false` rather
than omitting the keys — the daemon is then idle, with nothing to speak with.

The model is not here — read it at
[`GET /pipeline/{stage}/model`](./model.md#get-pipelinestagemodel). A settings
card that shows "Kokoro, kokoro-82m, on the GPU" is two reads, and they are
separate because the model read can be answering mid-download while the backend
selection has been settled since the user clicked.

**Errors:**

| HTTP | `error_code`      | Meaning                                 |
|------|-------------------|-----------------------------------------|
| 404  | `unknown_stage`   | No such position in the pipeline        |
| 401  | `invalid_session` | Token unknown / expired / `exe_changed` |
| 403  | `scope_denied`    | Token lacks the `settings` scope        |

## `POST /pipeline/{stage}`

Select the backend that fills this stage. Validates that it is installed and
serves this stage's role; does **not** load anything, and returns `200`.

The backends it will accept are
[`GET /pipeline/{stage}/backend/list`](./backend-list.md) — fill a picker from
that rather than from [`GET /backend/list`](../backends.md), or it offers
backends this refuses.

**Request:**

```http
POST /pipeline/1 HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
Content-Type: application/json

{ "source": "github.com/jorge-menjivar/super-tts-kokoro" }
```

| Field    | Type   | Required | Notes                                             |
|----------|--------|----------|---------------------------------------------------|
| `source` | string | yes      | Repo id of an installed backend serving this role |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "success",
  "stage": {
    "stage":   1,
    "role":    "synthesis",
    "source":  "github.com/jorge-menjivar/super-tts-kokoro",
    "name":    "Kokoro",
    "enabled": false
  }
}
```

Selecting a **different** backend drops the model with it — the name belonged
to the old backend, and two backends serving the same model name do not serve
the same model — and switches the stage off until a model is chosen, stopping
anything that model was saying. The daemon is then "backend selected, no model
loaded"; at startup such a state comes up idle, with nothing auto-loaded.
Re-selecting the backend already there changes nothing and interrupts nothing.

**Errors:**

| HTTP | `error_code`         | Meaning                                                          |
|------|----------------------|------------------------------------------------------------------|
| 404  | `unknown_stage`      | No such position in the pipeline                                 |
| 400  | `invalid_value`      | `source` missing or empty                                        |
| 400  | `invalid_backend`    | No installed backend with that `source` serves this stage's role, or its files are missing/invalid |
| 401  | `invalid_session`    | Token unknown / expired / `exe_changed`                          |
| 403  | `scope_denied`       | Token lacks the `settings` scope                                 |

## `DELETE /pipeline/{stage}`

Empty the stage: stop anything being spoken, unload the model, and forget it
along with the backend. For stage 1 that returns the daemon to fully idle — [`POST /speak`](../speak.md)
answers `409 model_not_loaded` until a backend and a model are chosen again.

Keeping the selection and only stopping the model is
[`DELETE /pipeline/{stage}/model`](./model.md#delete-pipelinestagemodel)
instead, which is what a Stop button should call: the user gets their memory
back without losing the choice they made.

**Request:**

```http
DELETE /pipeline/1 HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{ "status": "success" }
```

**Errors:** `404 unknown_stage`, plus the auth errors above.
