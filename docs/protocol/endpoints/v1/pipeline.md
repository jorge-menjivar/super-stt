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
| Read the backend | [`GET /pipeline/{stage}`](./pipeline/stage.md#get-pipelinestage)           | Which backend fills this stage, and whether it is switched on |
| Select backend   | [`POST /pipeline/{stage}`](./pipeline/stage.md#post-pipelinestage)         | Which installed backend fills this stage |
| Deselect backend | [`DELETE /pipeline/{stage}`](./pipeline/stage.md#delete-pipelinestage)       | Empty the stage, forgetting its model     |
| Read its model   | [`GET /pipeline/{stage}/model`](./pipeline/model.md#get-pipelinestagemodel)     | What it is pointed at, whether that is up, and the device it runs on |
| Run a model      | [`POST /pipeline/{stage}/model`](./pipeline/model.md#post-pipelinestagemodel)   | Load and run one of that backend's models |
| List its models  | [`GET /pipeline/{stage}/model/list`](./pipeline/model-list.md) | The models that backend serves in this stage's role |
| Read a model's language | [`GET /pipeline/{stage}/model/{model}/language`](./pipeline/language.md) | The language one of that backend's models transcribes in |
| Set it           | [`POST /pipeline/{stage}/model/{model}/language`](./pipeline/language.md#post-pipelinestagemodelmodellanguage) | Pin it, overriding the global setting |
| Stop it          | [`DELETE /pipeline/{stage}/model`](./pipeline/model.md#delete-pipelinestagemodel) | Unload, keeping the backend selected      |
| Read a model's device | [`GET /pipeline/{stage}/model/{model}/device`](./pipeline/device.md#get-pipelinestagemodelmodeldevice)  | Where one of that backend's models runs |
| Set it           | [`POST /pipeline/{stage}/model/{model}/device`](./pipeline/device.md#post-pipelinestagemodelmodeldevice) | Run it on the CPU or the GPU, reloading if it is loaded |
| List a model's devices | [`GET /pipeline/{stage}/model/{model}/device/list`](./pipeline/device.md#get-pipelinestagemodelmodeldevicelist) | What this install can run that model on |
| List the backend's devices | [`GET /pipeline/{stage}/device/list`](./pipeline/device.md#get-pipelinestagedevicelist) | What this install can run that backend on, for this stage |
| Abandon a load   | [`POST /pipeline/{stage}/model/cancel`](./pipeline/model.md#post-pipelinestagemodelcancel) | Stop the load this stage has in flight, download included |
| Reload in place  | [`POST /pipeline/{stage}/model/reload`](./pipeline/model.md#post-pipelinestagemodelreload) | Re-instantiate it to pick up changed secrets or options |

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
| `GET /pipeline/1`          | `GET /pipeline/1` (`stage.source`) plus `GET /pipeline/1/model` |
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
[`/pipeline/{stage}/model/{model}/device`](./pipeline/device.md#get-pipelinestagemodelmodeldevice).
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

A stage is a **backend selection**: which backend fills the position, and
whether the user has the position switched on. What that backend is *running* is
[the model object](#the-model-object), one level down.

| Field    | Type    | Notes                                                                   |
|----------|---------|--------------------------------------------------------------------------|
| `stage`  | int     | Its position, 1-based.                                                   |
| `role`   | string  | `transcription` or `post_processor`.                                     |
| `source` | string? | Repo id of the backend filling it; `null` when the stage is empty.       |
| `name`   | string? | That backend's display name; `null` when the stage is empty.            |
| `enabled`| bool    | Whether the user has this stage switched on — what Load sets and Unload clears. Distinct from `loaded`: a stage can be enabled while its model failed to come up, and transcripts then pass through unchanged. |

**The stages behave alike, and did not always.** Stage 1 used to report the
model it had *loaded* while stage 2 reported the model it had *selected*, so the
same field meant different things at the two positions — and at stage 1 `loaded`
was true exactly when `model` was non-null, telling a client nothing. Worse,
stage 1 had no `enabled` of its own, so the only way to stop a restart reloading
an unloaded model was to erase the selection: the card emptied, and re-loading
the same model on a different device meant picking it from the dropdown again.
Splitting the object and giving every stage an `enabled` is what fixed both.

## The model object

One stage's model slot, from
[`GET /pipeline/{stage}/model`](./pipeline/model.md#get-pipelinestagemodel).
Every stage answers it with the same shape and the same meanings.

| Field    | Type    | Notes                                                                   |
|----------|---------|--------------------------------------------------------------------------|
| `stage`  | int     | The position whose slot this is.                                         |
| `model`  | string? | Wire name of the selected model; `null` when none is picked. This is the **selection**, and it survives an unload — so a client can offer to load the same model again without the user re-picking it. |
| `loaded` | bool    | Whether that selection is running right now.                             |
| `device` | object? | Which accelerator it runs on; `null` when nothing is selected. See below.|
| `switch` | object? | The load this stage has in flight, or `null` when it has none. Scoped to the stage: a post-processor's download never appears under stage 1. It can be present while `model` is still `null` — that is a stage's very first load. |

### The `device` object

| Field            | Type    | Notes                                                            |
|------------------|---------|-------------------------------------------------------------------|
| `preference`     | string  | The stored choice: `cpu`, `gpu`, or `none` for a model that runs remotely. |
| `resolved_accel` | string? | What a `gpu` preference resolved to once the model loaded — `cuda`, `rocm`, `metal`, `vulkan`. `null` while the preference is `gpu` and nothing has confirmed it yet, so a client is never told a device resolved before a load proved it. |

What the model *could* run on is deliberately not here: that list costs a fresh
probe of the host's accelerators, and
[`GET /pipeline/{stage}/model/{model}/device/list`](./pipeline/device.md#get-pipelinestagemodelmodeldevicelist)
is the endpoint that answers it. Filling a device picker means one call for the
options and this one for the current value.

### The `switch` object

Present while a stage is provisioning a model — downloading its files, then
loading the weights.

| Field        | Type    | Notes                                                          |
|--------------|---------|-----------------------------------------------------------------|
| `phase`      | string  | `downloading`, `loading_model`, `completed`, `cancelled`, or `error` — the same vocabulary the `download_progress` event's `status` uses. |
| `target`     | object  | `{ model, source }` — what is being loaded, and the backend serving it. |
| `started_at` | string  | RFC 3339 timestamp of when the load began.                      |
| `download`   | object  | `{ current_file, file_index, total_files, bytes_downloaded, total_bytes, percentage, eta_seconds }`, per file — see [`download_progress`](./events.md#daemon-status) for what the counters mean. |

The polled mirror of the [events](#events) below: a client that wants live
progress subscribes, and one that reconnects mid-load reads it here.

Each verb's reference lives with its path:

| Page | Covers |
|---|---|
| [`pipeline/stage.md`](./pipeline/stage.md)   | `GET`, `POST`, `DELETE /pipeline/{stage}` |
| [`pipeline/model.md`](./pipeline/model.md)   | `GET`, `POST`, `DELETE /pipeline/{stage}/model`, and its `cancel` / `reload` |
| [`pipeline/model-list.md`](./pipeline/model-list.md) | `GET /pipeline/{stage}/model/list` |
| [`pipeline/device.md`](./pipeline/device.md) | `/pipeline/{stage}/model/{model}/device`, both device lists |
| [`pipeline/language.md`](./pipeline/language.md) | `/pipeline/{stage}/model/{model}/language` |

This page keeps what they share: the stages, the stage and model objects, auth,
the post-processing contract, and the events every stage emits.

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
      "stage":   1,
      "role":    "transcription",
      "source":  "github.com/super-stt/whisper",
      "name":    "Whisper (local)",
      "enabled": true
    },
    {
      "stage":   2,
      "role":    "post_processor",
      "source":  "github.com/jorge-menjivar/super-stt-textclean",
      "name":    "Text Cleanup",
      "enabled": true
    }
  ]
}
```

Which models those backends are running is one
[`GET /pipeline/{stage}/model`](./pipeline/model.md#get-pipelinestagemodel) per
position.

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

Every stage reports its own model lifecycle on
[`daemon_status_changed`](./events.md#daemon-status), and each of those events
carries the `stage` it is about — a client watching one stage must not read the
other's load as its own. Loading a model publishes `loading_model`, then
`model_switched` and `ready` once it is running; unloading publishes `ready`
with `model_loaded: false`. A load that downloads also publishes
[`download_progress`](./events.md#daemon-status) ticks, which carry the same
`stage` (and the `source` serving the model), so progress lands on the stage
that asked for it.

An event without a `stage` field comes from a daemon older than the field, and
is stage 1's: transcription was the only stage that emitted these.

Backend *selection* is separate from the model lifecycle: stage 1's publishes
`active_backend_changed`, stage 2's publishes `settings_changed` with
`setting: "post_processor"`. A device change that reloads stage 1's model
publishes `switching_device`, `loading_model_for_device` and then `ready` (or
`device_switch_error`); stage 2's device change is a plain reload and reports
itself as one. A device change that only records a choice publishes nothing.
