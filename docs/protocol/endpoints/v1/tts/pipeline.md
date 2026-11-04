# `/tts/pipeline`

The ordered **stages** an utterance passes through on its way from text to
audio. Text to speech has exactly one:

```
text ──▶ stage 1: synthesis ──▶ audio out
```

Stages are addressed **by position**, and every stage answers the same verbs,
so a client learns one shape and applies it anywhere in the pipeline:

| Verb | Path | Meaning |
|---|---|---|
| Read the backend | [`GET /tts/pipeline/{stage}`](./pipeline/stage.md#get-ttspipelinestage) | Which backend fills this stage, and whether it is switched on |
| List its backends | [`GET /tts/pipeline/{stage}/backend/list`](./pipeline/backend-list.md) | The installed backends that can fill this stage |
| Select backend | [`POST /tts/pipeline/{stage}`](./pipeline/stage.md#post-ttspipelinestage) | Which installed backend fills this stage |
| Deselect backend | [`DELETE /tts/pipeline/{stage}`](./pipeline/stage.md#delete-ttspipelinestage) | Empty the stage, forgetting its model |
| Read its model | [`GET /tts/pipeline/{stage}/model`](./pipeline/model.md#get-ttspipelinestagemodel) | What it is pointed at, whether that is up, and the device it runs on |
| Run a model | [`POST /tts/pipeline/{stage}/model`](./pipeline/model.md#post-ttspipelinestagemodel) | Load and run one of that backend's models |
| List its models | [`GET /tts/pipeline/{stage}/model/list`](./pipeline/model-list.md) | The models the backend filling this stage serves |
| Stop it | [`DELETE /tts/pipeline/{stage}/model`](./pipeline/model.md#delete-ttspipelinestagemodel) | Unload, keeping the backend selected |
| Abandon a load | [`POST /tts/pipeline/{stage}/model/cancel`](./pipeline/model.md#post-ttspipelinestagemodelcancel) | Stop the load this stage has in flight, download included |
| Reload in place | [`POST /tts/pipeline/{stage}/model/reload`](./pipeline/model.md#post-ttspipelinestagemodelreload) | Re-instantiate it to pick up changed secrets or options |
| Read a model's device | [`GET /tts/pipeline/{stage}/model/{model}/device`](./pipeline/device.md#get-ttspipelinestagemodelmodeldevice) | Where one of that backend's models runs |
| Set it | [`POST /tts/pipeline/{stage}/model/{model}/device`](./pipeline/device.md#post-ttspipelinestagemodelmodeldevice) | Run it on the CPU or the GPU, reloading if it is loaded |
| List a model's devices | [`GET /tts/pipeline/{stage}/model/{model}/device/list`](./pipeline/device.md#get-ttspipelinestagemodelmodeldevicelist) | What this install can run that model on |
| List the backend's devices | [`GET /tts/pipeline/{stage}/device/list`](./pipeline/device.md#get-ttspipelinestagedevicelist) | What this install can run that backend on, for this stage |
| Read a model's language | [`GET /tts/pipeline/{stage}/model/{model}/language`](./pipeline/language.md) | The language one of that backend's models speaks in |
| Set it | [`POST /tts/pipeline/{stage}/model/{model}/language`](./pipeline/language.md#post-ttspipelinestagemodelmodellanguage) | Pin it, overriding the global setting |
| List a model's languages | [`GET /tts/pipeline/{stage}/model/{model}/language/list`](./pipeline/language.md#get-ttspipelinestagemodelmodellanguagelist) | What that model can be pinned to |

`/tts/pipeline/{stage}` and `/tts/pipeline/{stage}/model` draw a deliberate split:
choosing a backend is cheap and cannot fail for runtime reasons, while loading a
model downloads, allocates and can fail. A backend selection outlives every
model that comes and goes under it, and a failed load leaves the selection
standing rather than returning the daemon to idle.

## The stages

| Stage | `role`      | What it does                                          |
|-------|-------------|-------------------------------------------------------|
| 1     | `synthesis` | Turns text into audio the daemon plays. Always present. |

**One stage, addressed by number anyway.** A single-stage pipeline does not
need positions — `/synthesis` would name the only thing there is. Numbering it
is what makes appending a second stage a new *position* rather than a new
endpoint family: a text pre-processor that expands abbreviations, normalizes
numbers or applies SSML before synthesis would slot in as a stage with its own
backend, model, device and language, and every verb above already spells its
address. A client written against `/tts/pipeline/1` today needs no new vocabulary
to drive `/tts/pipeline/2` tomorrow, and a client that hard-codes `1` keeps working
because the position it named did not move.

**`GET /tts/pipeline/2` answers `404 unknown_stage`** today, with a message naming
the stages that do exist. That is the honest answer for a position the daemon
does not have, and the one a client should surface as "this build has one
stage" rather than as a transport failure.

Two things are settled now rather than later. A stage's `role` travels with it,
so the daemon can reject a nonsensical composition — a model declaring one
role cannot be loaded into a position expecting another — rather than
discovering it mid-utterance. And the list is *ordered*, so "what runs before
what" has an answer that does not depend on the order a client happened to
configure things in. When stages become insertable, renumbering is the thing to
design: a client holding "stage 2" must not silently end up pointed at a
different model.

## What this replaces

| Was | Now |
|---|---|
| `GET /active_backend`        | [`GET /tts/pipeline/1`](./pipeline/stage.md#get-ttspipelinestage) (`stage.source`) plus [`GET /tts/pipeline/1/model`](./pipeline/model.md#get-ttspipelinestagemodel) |
| `POST /active_backend`       | [`POST /tts/pipeline/1`](./pipeline/stage.md#post-ttspipelinestage)   |
| `DELETE /active_backend`     | [`DELETE /tts/pipeline/1`](./pipeline/stage.md#delete-ttspipelinestage) |
| `GET /active_model`          | [`GET /tts/pipeline/1/model`](./pipeline/model.md#get-ttspipelinestagemodel) |
| `POST /active_model`         | [`POST /tts/pipeline/1/model`](./pipeline/model.md#post-ttspipelinestagemodel) |
| `DELETE /active_model`       | [`DELETE /tts/pipeline/1/model`](./pipeline/model.md#delete-ttspipelinestagemodel) |
| `POST /active_model/cancel`  | [`POST /tts/pipeline/1/model/cancel`](./pipeline/model.md#post-ttspipelinestagemodelcancel) |
| `POST /active_model/reload`  | [`POST /tts/pipeline/1/model/reload`](./pipeline/model.md#post-ttspipelinestagemodelreload) |
| `GET /models`                | [`GET /tts/pipeline/1/model/list`](./pipeline/model-list.md)       |
| `GET`/`POST /active_device`  | [`/tts/pipeline/1/model/{model}/device`](./pipeline/device.md)     |
| `/backends/{source}/models/{model}/language` | [`/tts/pipeline/1/model/{model}/language`](./pipeline/language.md) |

The stage object is flatter than the old `active_model` payload: `current.model`
and `current.source` are `model` and `source`, and `current.provider` — a
compatibility shim that was always an empty string — is gone.

**`/active_device` is gone.** It set one device for the whole daemon; a device
is a property of a model — a small preset-voice model runs fine on the CPU
while the cloning model beside it needs the GPU — so it is now read and set per
model, at
[`/tts/pipeline/{stage}/model/{model}/device`](./pipeline/device.md#get-ttspipelinestagemodelmodeldevice).
A daemon upgraded from the global preference keeps loading models where it
always did: a model with no device of its own falls back to the old global
value in `daemon.ron`, and takes its own the first time one is set.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## The stage object

A stage is a **backend selection**: which backend fills the position, and
whether the user has the position switched on. What that backend is *running*
is [the model object](#the-model-object), one level down.

| Field     | Type    | Notes                                                                  |
|-----------|---------|------------------------------------------------------------------------|
| `stage`   | int     | Its position, 1-based.                                                 |
| `role`    | string  | `synthesis`. The only role this build defines.                         |
| `source`  | string? | Repo id of the backend filling it; `null` when the stage is empty.     |
| `name`    | string? | That backend's display name; `null` when the stage is empty.           |
| `enabled` | bool    | Whether the user has this stage switched on — what a model load sets and an unload clears. Distinct from `loaded`: a stage can be enabled while its model failed to come up, and [`POST /tts/speak`](./speak.md) then answers `409 model_not_loaded`. |

**Why `enabled` is separate from `model`.** Without it, the only way to stop a
restart from reloading a model that had just been unloaded was to erase the
selection entirely: the settings card emptied, and loading the same model again
on a different device meant picking it out of the dropdown a second time.
Keeping the selection and switching the stage off says "this is what I want,
just not right now", which is what the user meant.

## The model object

One stage's model slot, from
[`GET /tts/pipeline/{stage}/model`](./pipeline/model.md#get-ttspipelinestagemodel).

| Field    | Type    | Notes                                                                   |
|----------|---------|-------------------------------------------------------------------------|
| `stage`  | int     | The position whose slot this is.                                        |
| `model`  | string? | Wire name of the selected model; `null` when none is picked. This is the **selection**, and it survives an unload — so a client can offer to load the same model again without the user re-picking it. |
| `loaded` | bool    | Whether that selection is running right now. This is what [`POST /tts/speak`](./speak.md) needs: a stage whose `loaded` is `false` refuses an utterance. |
| `device` | object? | Which accelerator it runs on; `null` when nothing is selected. See below. |
| `switch` | object? | The load this stage has in flight, or `null` when it has none. Scoped to the stage. It can be present while `model` is still `null` — that is a stage's very first load. |

### The `device` object

| Field            | Type    | Notes                                                              |
|------------------|---------|--------------------------------------------------------------------|
| `preference`     | string  | The stored choice: `cpu`, `gpu`, or `none` for a model that runs remotely. |
| `resolved_accel` | string? | What a `gpu` preference resolved to once the model loaded — `cuda`, `rocm`, `metal`, `vulkan`. `null` while the preference is `gpu` and nothing has confirmed it yet, so a client is never told a device resolved before a load proved it. |

What the model *could* run on is deliberately not here: that list costs a fresh
probe of the host's accelerators, and
[`GET /tts/pipeline/{stage}/model/{model}/device/list`](./pipeline/device.md#get-ttspipelinestagemodelmodeldevicelist)
is the endpoint that answers it. Filling a device picker means one call for the
options and this one for the current value.

### The `switch` object

Present while a stage is provisioning a model — downloading its files, then
loading the weights.

| Field        | Type   | Notes                                                            |
|--------------|--------|------------------------------------------------------------------|
| `phase`      | string | `verifying`, `downloading`, `loading_model`, `completed`, `cancelled`, or `error` — the same vocabulary the `download_progress` event's `status` uses. |
| `target`     | object | `{ model, source }` — what is being loaded, and the backend serving it. |
| `started_at` | string | RFC 3339 timestamp of when the load began.                       |
| `download`   | object | `{ current_file, file_index, total_files, bytes_downloaded, total_bytes, percentage, eta_seconds }`, per file — see [`download_progress`](../events.md) for what the counters mean. |

The polled mirror of the [events](#events) below: a client that wants live
progress subscribes, and one that reconnects mid-load reads it here.

## Where each verb is documented

| Page | Covers |
|---|---|
| [`pipeline/stage.md`](./pipeline/stage.md) | `GET`, `POST`, `DELETE /tts/pipeline/{stage}` |
| [`pipeline/backend-list.md`](./pipeline/backend-list.md) | `GET /tts/pipeline/{stage}/backend/list` |
| [`pipeline/model.md`](./pipeline/model.md) | `GET`, `POST`, `DELETE /tts/pipeline/{stage}/model`, and its `cancel` / `reload` |
| [`pipeline/model-list.md`](./pipeline/model-list.md) | `GET /tts/pipeline/{stage}/model/list` |
| [`pipeline/device.md`](./pipeline/device.md) | `/tts/pipeline/{stage}/model/{model}/device`, and both device lists |
| [`pipeline/language.md`](./pipeline/language.md) | `/tts/pipeline/{stage}/model/{model}/language`, and the list it offers |

This page keeps what they share: the stages, the stage and model objects, auth,
and the events every stage emits.

## `GET /tts/pipeline`

Every stage, in order.

**Request:**

```http
GET /tts/pipeline HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "success",
  "pipeline": [
    {
      "stage":   1,
      "role":    "synthesis",
      "source":  "github.com/jorge-menjivar/super-tts-kokoro",
      "name":    "Kokoro",
      "enabled": true
    }
  ]
}
```

A one-element array, today and until a second position exists. A client renders
the array rather than indexing `[0]` and calling it "the model": the array is
what grows.

Which model that backend is running is
[`GET /tts/pipeline/1/model`](./pipeline/model.md#get-ttspipelinestagemodel) — one
call per position, deliberately not folded in here, because a stage report
should not have to wait on a load in flight.

**Errors:**

| HTTP | `error_code`      | Meaning                                 |
|------|-------------------|-----------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed` |
| 403  | `scope_denied`    | Token lacks the `settings` scope        |

## `404 unknown_stage`

Every path under `/tts/pipeline/{stage}` resolves the position first. A position
this build does not have is `404 unknown_stage`, whatever the rest of the path
says:

```http
GET /tts/pipeline/2 HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
```

```http
HTTP/1.1 404 Not Found
Content-Type: application/json

{
  "status":     "error",
  "error_code": "unknown_stage",
  "message":    "No stage 2; this pipeline has stage 1 (synthesis)"
}
```

The message names the stages that do exist, so a client discovering the
pipeline's shape gets it from the error rather than from a second request.
`GET /tts/pipeline` is the deliberate way to ask.

## Events

The stage reports its model lifecycle on
[`daemon_status_changed`](../events.md). Loading a model publishes
`loading_model`, then `model_switched` and `ready` once it is running;
unloading publishes `ready` with `model_loaded: false`. A load that downloads
also publishes `download_progress` ticks, which carry the `source` serving the
model.

Backend *selection* is separate from the model lifecycle and publishes
`active_backend_changed`, carrying the new `source` — or `null` when the stage
is emptied. A device change that reloads the stage's model publishes
`switching_device`, `loading_model_for_device` and then `ready` (or
`device_switch_error`); a device change that only records a choice for a model
that is not loaded publishes nothing.

**No `stage` field, yet.** With one position, every one of these events is
stage 1's and the field would be a constant. It is the piece a second stage
would have to add first: two stages provisioning independently means a client
watching one must not read the other's load as its own, and an event that does
not say which position it came from cannot be routed. A client that wants to be
ready for it treats a missing `stage` as `1` rather than as "unknown".
