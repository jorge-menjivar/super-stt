# `/pipeline`

The ordered **stages** a transcript passes through. Stage 1 turns audio into
text; every later stage rewrites the text the stage before it produced.

```
audio ──▶ stage 1: transcription ──▶ stage 2: post-processing ──▶ typed text
```

Stages are addressed **by position**, and every stage answers the same verbs,
so a client learns one shape and applies it anywhere in the pipeline:

| Verb | Path | Meaning |
|---|---|---|
| Select backend   | `POST /pipeline/{stage}`         | Which installed backend fills this stage |
| Deselect backend | `DELETE /pipeline/{stage}`       | Empty the stage, forgetting its model     |
| Run a model      | `POST /pipeline/{stage}/model`   | Load and run one of that backend's models |
| Stop it          | `DELETE /pipeline/{stage}/model` | Unload, keeping the backend selected      |
| Read a model's device | `GET /pipeline/{stage}/model/{model}/device`  | Where one of that backend's models runs |
| Set it           | `POST /pipeline/{stage}/model/{model}/device` | Run it on the CPU or the GPU, reloading if it is loaded |
| List a model's devices | `GET /pipeline/{stage}/model/{model}/device/list` | What this install can run that model on |
| List the backend's devices | `GET /pipeline/{stage}/device/list` | What this install can run that backend on, for this stage |

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

**`/active_device` is gone too.** It set one device for the whole daemon; a
device is a property of a model — a small one runs fine on the CPU while the
large one beside it needs the GPU, and a post-processor sharing the pipeline
with either has its own answer again — so it is now read and set per model, at
[`/pipeline/{stage}/model/{model}/device`](#get-pipelinestagemodelmodeldevice).
A daemon upgraded from the global preference keeps loading models where it
always did: a model with no device of its own falls back to the old global
value in `daemon.toml`, and takes its own the first time one is set.

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
| `device` | string? | The accelerator the loaded model is actually on: `cpu`, `cuda`, `rocm`, `metal`, `vulkan`, or `remote` for an online model. `null` when nothing is loaded. This is where the work runs, not the user's preference — for that, and for what a `gpu` choice resolved to, read the model's [device](#get-pipelinestagemodelmodeldevice). |
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
      "stage":   2,
      "role":    "post_processor",
      "source":  "github.com/jorge-menjivar/super-stt-textclean",
      "name":    "Text Cleanup",
      "model":   "textclean",
      "loaded":  true,
      "device":  "cpu",
      "enabled": true
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
    "stage":   2,
    "role":    "post_processor",
    "source":  "github.com/jorge-menjivar/super-stt-textclean",
    "name":    "Text Cleanup",
    "model":   "textclean",
    "loaded":  true,
    "device":  "cpu",
    "enabled": true
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

{ "model": "textclean" }
```

| Field    | Type   | Required | Notes                                                                    |
|----------|--------|----------|--------------------------------------------------------------------------|
| `model`  | string | yes      | A model its backend declares with this stage's `role`                    |
| `source` | string | no       | Repo id of the serving backend. Omitted resolves to the backend selected for this stage. |

The model loads on its own [device](#get-pipelinestagemodelmodeldevice), which
is not part of this request: set it first, and it is remembered for every
later load of that model.

Stage 1 answers `202 Accepted` and switches asynchronously (see
[`/pipeline/1/model`](./pipeline.md)); stage 2 answers `200` once the load has
been attempted. A stage-2 load failure still succeeds — the setting is saved and
the reason is appended to `message` — because post-processing degrades to
passing text through rather than costing the user their words.

**Errors:**

| HTTP | `error_code`             | Meaning                                                              |
|------|--------------------------|----------------------------------------------------------------------|
| 404  | `unknown_stage`          | No such position in the pipeline                                     |
| 400  | `invalid_value`          | `model` missing or empty. To stop a stage, use `DELETE`.             |
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

## `GET /pipeline/{stage}/model/{model}/device`

The device `model` runs on. `model` is resolved against the backend selected
for this stage — the same resolution an omitted `source` gets on
`POST /pipeline/{stage}/model` — and must carry this stage's `role`.

**Request:**

```http
GET /pipeline/1/model/whisper-large-v3/device HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":            "success",
  "device":            "gpu",
  "resolved_accel":    "cuda",
  "available_devices": ["cpu", "gpu"]
}
```

| Field               | Type     | Notes                                                                                   |
|---------------------|----------|-----------------------------------------------------------------------------------------|
| `device`            | string   | The device the model loads on, `"cpu"` or `"gpu"` — its own, or the daemon's default when it has none of its own. `"none"` for an online model, which runs remotely and has no local device. |
| `resolved_accel`    | string?  | What that preference resolved to: `"cuda"`, `"rocm"`, `"metal"`, `"vulkan"` or `"cpu"` while the model is loaded in this stage — always what actually loaded, so a `"gpu"` choice that fell back reads `"cpu"` here. When the model is not loaded: `"cpu"` for a `"cpu"` preference (nothing to resolve), `null` for `"gpu"` (nothing has resolved yet). `null` for an online model. |
| `available_devices` | string[] | The devices this install can offer the model on this host: the model's `supported_devices`, narrowed to the accelerators its installed asset actually provides (`installed_accel` on [`GET /backends`](./backends.md)) and to what the host has. A CUDA-only backend on a host without an NVIDIA GPU installed its CPU asset, and is not offered a GPU here. Empty for an online model. |

**Errors:**

| HTTP | `error_code`      | Meaning                                                                    |
|------|-------------------|----------------------------------------------------------------------------|
| 404  | `unknown_stage`   | No such position in the pipeline                                           |
| 400  | `invalid_backend` | No backend is selected for this stage, so there is nothing to resolve `model` against |
| 400  | `invalid_model`   | The selected backend serves no `model`, or `model` carries the other stage's role — the message names the stage that runs it |
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                                    |
| 403  | `scope_denied`    | Token lacks the `settings` scope                                           |

## `POST /pipeline/{stage}/model/{model}/device`

Run `model` on `device`. For the model loaded in this stage this is a reload
onto the new device; for any other it only records the choice, which the
model's next load picks up. Either way the choice is the model's own from
then on: it is remembered per `(source, model)`, so two models on one backend
can live on different devices.

**Request:**

```http
POST /pipeline/1/model/whisper-large-v3/device HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{ "device": "gpu" }
```

| Field    | Type   | Required | Notes                                                                                                    |
|----------|--------|----------|--------------------------------------------------------------------------------------------------------|
| `device` | string | yes      | `"cpu"` or `"gpu"`. `"cuda"` and `"metal"` are accepted input spellings, normalized to `"gpu"`. Any other value is rejected. |

**Response (200):** the same body `GET` answers with, reflecting the new
state. `message` says which of the three things happened: recorded for the
next load, reloaded, or already on that device.

The reload, when there is one, completes before the response: stage 1 unloads
the model, publishes `switching_device` / `loading_model_for_device` on
[`/events`](./events.md), reloads, and publishes `ready` with the resolved
`actual_device`. If the reload fails the model is put back on its previous
device, its setting is left as it was, and the call fails with the reason.
Stage 2 follows the post-processor's best-effort policy instead: the setting
is saved, the reload is attempted, and a failure is appended to `message`
while the previous instance keeps running.

Asking for the device a loaded model is already on reloads nothing — a `gpu`
preference already resolved to `cuda` is the same choice spelled twice — but a
`gpu` preference that fell back to the CPU is retried, which is what tracking
the preference and the resolved accelerator separately is for.

**Errors:** those of `GET` above, plus:

| HTTP | `error_code`            | Meaning                                                                    |
|------|-------------------------|----------------------------------------------------------------------------|
| 400  | `invalid_device`        | `device` is not `"cpu"`, `"gpu"` or an accepted alias; or the model's manifest does not declare it (a CPU-only model cannot be sent to the GPU); or the model is online and has no local device. Nothing is recorded. |
| 409  | `recording_in_progress` | A reload is needed and a recording is in flight                            |

Requesting `"gpu"` for a model whose install or host cannot provide one is
**not** an error — only the manifest is consulted — and the load falls back to
the CPU, surfaced as `resolved_accel: "cpu"` and the `ready` event's
`actual_device`. A client that wants to offer only what will work narrows its
picker to `available_devices`.

## `GET /pipeline/{stage}/model/{model}/device/list`

The devices this install can offer `model` on this host — the
`available_devices` of the model's [device](#get-pipelinestagemodelmodeldevice),
on its own, for a client that only wants to fill a picker. `model` resolves
the same way, and the same errors apply.

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":            "success",
  "available_devices": ["cpu", "gpu"]
}
```

Empty for an online model, and for a local model this install cannot run on
any device (a GPU-only model whose installed asset is CPU-only).

## `GET /pipeline/{stage}/device/list`

The devices the backend selected for this stage can be run on here: the union
of the list above over the models it serves **for this stage's role**. A
backend serving both a transcription model and a post-processor answers stage
1 for the former and stage 2 for the latter, since that is what "this backend"
means from a stage. Always in `cpu`, `gpu` order.

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":            "success",
  "available_devices": ["cpu", "gpu"]
}
```

**Errors:** `404 unknown_stage`; `400 invalid_backend` when no backend is
selected for the stage; plus the auth errors above.

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
`setting: "post_processor"`. A device change that reloads stage 1's model
publishes `switching_device`, `loading_model_for_device` and then `ready` (or
`device_switch_error`); one that only records a choice publishes nothing.
