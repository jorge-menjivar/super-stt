# `/pipeline`

The ordered **stages** a transcript passes through. Stage 1 turns audio into
text; every later stage rewrites the text the stage before it produced.

```
audio ──▶ stage 1: transcription ──▶ stage 2: post-processing ──▶ typed text
```

Stages are addressed **by position**, and every stage answers the same four
verbs, so a client learns one shape and applies it anywhere in the pipeline:

| Verb | Path | Meaning |
|---|---|---|
| Select backend   | `POST /pipeline/{stage}`         | Which installed backend fills this stage |
| Deselect backend | `DELETE /pipeline/{stage}`       | Empty the stage, forgetting its model     |
| Run a model      | `POST /pipeline/{stage}/model`   | Load and run one of that backend's models |
| Stop it          | `DELETE /pipeline/{stage}/model` | Unload, keeping the backend selected      |

That split is the same one `/pipeline/1` and `/pipeline/1/model` draw for
transcription, and for the same reason: choosing a backend is cheap and cannot
fail for runtime reasons, while loading a model downloads, allocates and can
fail.

## The stages

| Stage | `role`            | What it does                                                     |
|-------|-------------------|------------------------------------------------------------------|
| 1     | `transcription`   | Turns recorded audio into text. Always present.                  |
| 2     | `post_processor`  | Rewrites each final transcript — fillers, punctuation, formatting.|

A stage's `role` must match the model's
[`role`](../../backend/config.md#models) in its backend's manifest, so a
transcription model cannot be run in stage 2 and a post-processor cannot be run
in stage 1.

**This replaces `/pipeline/1` and `/pipeline/1/model`,** which were the
stage-1-only spelling of the same four verbs and have been removed. A client
that used them moves as follows:

| Was | Now |
|---|---|
| `GET /pipeline/1`          | `GET /pipeline/1` (`stage.source`)      |
| `POST /pipeline/1`         | `POST /pipeline/1`                      |
| `DELETE /pipeline/1`       | `DELETE /pipeline/1`                    |
| `GET /pipeline/1`            | `GET /pipeline/1`                       |
| `POST /pipeline/1/model`           | `POST /pipeline/1/model`                |
| `DELETE /pipeline/1/model`         | `DELETE /pipeline/1/model`              |
| `POST /pipeline/1/model/cancel`    | `POST /pipeline/1/model/cancel`         |
| `POST /pipeline/1/model/reload`    | `POST /pipeline/1/model/reload`         |

The stage object is flatter than the old `active_model` payload: `current.model`
and `current.source` are `model` and `source`, and `current.provider` — a
compatibility shim that was always an empty string — is gone.

**On extending the pipeline.** Positions are the contract, so a third stage is
a new position rather than a new endpoint. Two things are deliberately settled
now and not later: a stage's `role` travels with it (so the daemon can reject a
nonsensical composition rather than discovering it at runtime), and the list is
ordered (so "which post-processor runs first" has an answer). Today the length
is fixed at two, and an out-of-range stage is `404 unknown_stage`; when stages
become insertable, renumbering is the thing to design — a client holding
"stage 3" must not silently end up pointed at a different model.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## The stage object

| Field    | Type    | Notes                                                                   |
|----------|---------|--------------------------------------------------------------------------|
| `stage`  | int     | Its position, 1-based.                                                   |
| `role`   | string  | `transcription` or `post_processor`.                                     |
| `source` | string? | Repo id of the backend filling it; `null` when the stage is empty.       |
| `name`   | string? | That backend's display name; `null` when the stage is empty.            |
| `model`  | string? | Wire name of the selected model; `null` when none is selected.          |
| `loaded` | bool    | Whether the model is loaded and running right now.                       |
| `device` | string? | Where the work runs: the accelerator the loaded model is actually on — `cpu`, `cuda`, `rocm`, `metal`, `vulkan`, or `remote` for a cloud model. `null` until a model has loaded. |
| `preferred_device` | string? | *Processor stages only.* The stage's own `cpu`/`gpu` ask, set with [`POST /pipeline/{stage}/model`](#post-pipelinestagemodel); `device` is what it resolved to. `null` when the stage has none and follows stage 1's, whose preference lives at [`/active_device`](./active_device.md). |
| `enabled`| bool    | *Processor stages only.* Whether it should run. Distinct from `loaded`: a stage can be enabled while its model failed to load, and transcripts then pass through unchanged. |

## `GET /pipeline`

Every stage, in order.

**Request:**

```http
GET /pipeline HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "success",
  "pipeline": [
    {
      "stage":  1,
      "role":   "transcription",
      "source": "github.com/super-stt/whisper",
      "name":   "Whisper (local)",
      "model":  "whisper-large-v3",
      "loaded": true,
      "device": "cuda"
    },
    {
      "stage":            2,
      "role":             "post_processor",
      "source":           "github.com/jorge-menjivar/super-stt-textclean",
      "name":             "Text Cleanup",
      "model":            "textclean",
      "loaded":           true,
      "device":           "cpu",
      "preferred_device": "cpu",
      "enabled":          true
    }
  ]
}
```

## `GET /pipeline/{stage}`

One stage, as `stage` — the same object the array carries.

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "success",
  "stage": {
    "stage":            2,
    "role":             "post_processor",
    "source":           "github.com/jorge-menjivar/super-stt-textclean",
    "name":             "Text Cleanup",
    "model":            "textclean",
    "loaded":           true,
    "device":           "cpu",
    "preferred_device": "cpu",
    "enabled":          true
  }
}
```

## `POST /pipeline/{stage}`

Select the backend that fills this stage. Validates that it is installed and
serves this stage's role; does **not** load anything.

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
the old backend — and stops that stage until a model is chosen. Re-selecting the
backend already there changes nothing.

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

## `POST /pipeline/{stage}/model`

Run a model in this stage.

**Request:**

```http
POST /pipeline/2/model HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{ "model": "textclean", "device": "gpu" }
```

| Field    | Type   | Required | Notes                                                                    |
|----------|--------|----------|--------------------------------------------------------------------------|
| `model`  | string | yes      | A model its backend declares with this stage's `role`                    |
| `source` | string | no       | Repo id of the serving backend. Omitted resolves to the backend selected for this stage. |
| `device` | string | no       | *Processor stages only.* The stage's own `cpu`/`gpu` preference, normalized as [`/active_device`](./active_device.md) normalizes (`cuda`/`metal` → `gpu`). Omitted keeps the stored one; a stage that has never been given one follows stage 1's. Stage 1 refuses the field (`400 invalid_value`): its device is the daemon-wide preference at `/active_device`, with a reload of its own. |

A processor stage runs beside the transcription model, so it gets hardware
chosen for it: a small cleanup model can sit on the CPU while a large
transcription model has the GPU, or the reverse.

Stage 1 answers `202 Accepted` and switches asynchronously (see
[`/pipeline/1/model`](./pipeline.md)); stage 2 answers `200` once the load has
been attempted. A stage-2 load failure still succeeds — the setting is saved and
the reason is appended to `message` — because post-processing degrades to
passing text through rather than costing the user their words.

**Errors:**

| HTTP | `error_code`             | Meaning                                                              |
|------|--------------------------|----------------------------------------------------------------------|
| 404  | `unknown_stage`          | No such position in the pipeline                                     |
| 400  | `invalid_value`          | `model` missing or empty (to stop a stage, use `DELETE`), or `device` sent to a stage that does not take one |
| 400  | `invalid_device`         | `device` is not `cpu` or `gpu`                                       |
| 400  | `invalid_model`          | No installed backend serves `(model, source)`, or its role does not match this stage |
| 400  | `invalid_backend`        | `source` omitted and no backend is selected for this stage           |
| 400  | `online_models_disabled` | The model is online and online models are disabled                   |
| 409  | `recording_in_progress`  | A recording is in flight                                             |
| 401  | `invalid_session`        | Token unknown / expired / `exe_changed`                              |
| 403  | `scope_denied`           | Token lacks the `settings` scope                                     |

## `DELETE /pipeline/{stage}/model`

Stop this stage, **keeping its backend selected** so restarting it is one
`POST`. No-op when the stage is not running.

For stage 1 this is [`DELETE /pipeline/1/model`](./pipeline.md) — the model
unloads, the backend stays active. For stage 2 it turns post-processing off
while remembering the chosen model.

**Errors:** `404 unknown_stage`, `409 recording_in_progress`, plus the auth
errors above.

## `POST /pipeline/{stage}/model/cancel`

Abandon a load that is still in flight, including the download feeding it.

Only stages that can be interrupted implement it: stage 1 does, and stage 2
answers `404 unsupported_action` because a post-processor load is not something
there is a partial download to abandon.

**Errors:** `409` when no load is in progress (`No download in progress`),
`404 unknown_stage`, `404 unsupported_action`, plus the auth errors above.

## `POST /pipeline/{stage}/model/reload`

Re-instantiate this stage's model in place, picking up changed
[secrets and options](./backends.md) without a manual stop/start.

Stage 1 implements it; stage 2 answers `404 unsupported_action` — re-running
`POST /pipeline/2/model` does the same job for a post-processor today.

**Errors:** `404 unknown_stage`, `404 unsupported_action`,
`409 recording_in_progress`, plus the auth errors above.

## Post-processing behavior

**Best-effort.** If a post-processing stage is unloaded, errors, or takes longer
than 30 seconds, the **raw transcript is delivered unchanged** and the failure is
logged. A cleanup step that is down never costs the user their words.

**What it applies to:** every final transcript — the daemon's own recording flow
and [`POST /transcribe`](./transcribe.md). It deliberately does **not** apply to
preview text or to [realtime sessions](../../backend/contract.md#consumer-facing-endpoint): both are
latency paths, and a preview is rewritten again on the next pass anyway.

A post-processor is driven over
[`POST /v1/process`](../../backend/contract.md#post-v1process); what it does with
the text, and how it is tuned, is the backend's own business, configured through
its [options and secrets](./backends.md) like any other model.

## Events

A change to any stage publishes `daemon_status_changed` on
[`/events`](./events.md): stage 1 through the existing `active_backend_changed` /
`model_switched` variants, stage 2 as `settings_changed` with
`setting: "post_processor"`.
