# `/pipeline/{stage}/model`

The model one [stage](./stage.md) is pointed at: read it, run it, stop it,
abandon a load still in flight, or re-instantiate it in place. This is the
endpoint family a "Model" settings card is built from, and the one
[`POST /speak`](../speak.md) depends on — an utterance needs a stage whose
model is `loaded`.

All of them are scoped to the stage in the path. Stages provision
independently, so one stage's cancel is never a licence to abandon another's
download.

Emptying the stage entirely — forgetting the backend along with the model — is
[`DELETE /pipeline/{stage}`](./stage.md#delete-pipelinestage) instead. The
[auth](../pipeline.md#auth), the [model object](../pipeline.md#the-model-object)
and its [`device`](../pipeline.md#the-device-object) and
[`switch`](../pipeline.md#the-switch-object) sub-objects are described once in
the family overview.

A model is named by a `(model, source)` pair:

- **`model`** — `kokoro-82m`, `gpt-4o-mini-tts`, `qwen3-tts-1.7b-base`, …
- **`source`** — the repo id of the backend serving it. Omitted, it resolves to
  the backend filling this stage; with no backend selected the call fails with
  `400 invalid_backend`.

Two backends may serve the same `name`, so resolving an omitted `source` by
scanning for whichever backend happens to serve it would load an engine the
caller did not ask for — and that choice is persisted. Select the backend first
with [`POST /pipeline/{stage}`](./stage.md#post-pipelinestage), or name
`source` explicitly.

## `GET /pipeline/{stage}/model`

What this stage is pointed at, whether it is running, the accelerator it runs
on, and the load still in flight — in one payload, so a settings UI renders the
whole Model section from a single request.

**Request:**

```http
GET /pipeline/1/model HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
```

**Response (200):**

```jsonc
{
  "status": "success",
  "model": {
    "stage":  1,
    "model":  "kokoro-82m",
    "loaded": true,
    "device": {
      "preference":     "gpu",
      "resolved_accel": "cuda"
    },
    "switch": null
  }
}
```

`model` is the **selection**, not the running instance: it survives an
[unload](#delete-pipelinestagemodel), so a client can offer to load the same
model again — onto another device, say — without the user picking it a second
time. `loaded` is what says whether it is up, and what a Speak button should be
gated on.

An empty slot reports `null` rather than omitting the keys:

```json
{ "stage": 1, "model": null, "loaded": false, "device": null, "switch": null }
```

While a load is in flight, `switch` is populated and `model`/`loaded` still
describe what was there before — the previous model keeps speaking until the
new one is up. What the loaded model can *do* — its voices, its input cap, its
output rate — is not here: that is the model's manifest entry on
[`GET /backend/list`](../backends.md), plus the loaded-model summary
[`GET /voice/list`](../voice.md#get-voicelist) carries for a voices UI.

**Phase values** (`switch.phase`):

| Value           | Meaning                                                                |
|-----------------|------------------------------------------------------------------------|
| `verifying`     | Checking the model files already on disk (size, and the declared SHA-256) before fetching anything. Every load opens here, and a load with nothing to fetch never leaves it, so a client must not word this phase as a download. The `download` sub-object is populated, counting bytes hashed rather than bytes received. |
| `downloading`   | Pulling model files; the `download` sub-object is populated.            |
| `loading_model` | Files are in place; weights are being loaded onto the chosen device.    |
| `completed`     | The load finished; cleared on the next read after `ready`.              |
| `cancelled`     | [`POST …/model/cancel`](#post-pipelinestagemodelcancel) interrupted it. |
| `error`         | The load failed (network, disk, hash mismatch, a missing secret, …).    |

**Errors:** `404 unknown_stage`, plus the auth errors below.

## `POST /pipeline/{stage}/model`

Run a model in this stage. The call returns as soon as the switch is *started*,
not when it finishes: a load downloads weights and allocates a device, which can
take minutes. The status is `200`, so it does not tell you that — the
`"Model switch started"` message does, and progress is visible by polling
[`GET /pipeline/{stage}/model`](#get-pipelinestagemodel) or, better, by
subscribing to
[`GET /events?topics=daemon_status_changed,download_progress`](../events.md).

**Request:**

```http
POST /pipeline/1/model HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
Content-Type: application/json

{
  "model":  "kokoro-82m",
  "source": "github.com/jorge-menjivar/super-tts-kokoro"
}
```

| Field    | Type   | Required | Notes                                                                    |
|----------|--------|----------|--------------------------------------------------------------------------|
| `model`  | string | yes      | A model the stage's backend serves, as [`GET /pipeline/{stage}/model/list`](./model-list.md) spells it |
| `source` | string | no       | Repo id of the serving backend. Omitted resolves to the backend selected for this stage. |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{ "status": "success", "message": "Model switch started" }
```

A `200` here means "the switch was accepted and has begun", never "the model is
loaded". Read [`GET /pipeline/{stage}/model`](#get-pipelinestagemodel) for that:
`loaded` is the field that answers it, and `switch` reports the operation still
in flight.

Naming a `source` also selects that backend for the stage, so a client can go
straight from a flat model picker to a load. If the model then fails to
load — a missing secret, a download that could not complete — the backend stays
selected and **no model is loaded**: the daemon does not silently restore the
model that was running, because a client that asked for a switch and got a
success would otherwise be speaking in the old voice without being told.

Anything being spoken is stopped when the switch starts: the switch is the newer
request, the same way a second [`POST /speak`](../speak.md) supersedes the
first. Asking for the model that is already running changes nothing and
interrupts nothing.

The model loads on its own
[device](./device.md#get-pipelinestagemodelmodeldevice), which is not part of
this request: set it first, and it is remembered for every later load of that
model. Changing the device of a model that is **already running** does not need
this endpoint at all — `POST` the new device and the daemon reloads it in
place.

**Errors:**

| HTTP | `error_code`             | Meaning                                                                      |
|------|--------------------------|------------------------------------------------------------------------------|
| 404  | `unknown_stage`          | No such position in the pipeline                                             |
| 400  | `invalid_value`          | `model` missing or empty. To stop a stage, use `DELETE`.                     |
| 400  | `invalid_model`          | No installed backend serves `(model, source)`, or its role does not match this stage |
| 400  | `invalid_backend`        | `source` omitted and no backend is selected for this stage                   |
| 400  | `online_models_disabled` | The model is online and [`allow_online_models`](../settings/allow_online_models.md) is `false` |
| 409  | `switch_in_progress`     | This stage already has a load in flight — cancel it or wait                  |
| 401  | `invalid_session`        | Token unknown / expired / `exe_changed`                                      |
| 403  | `scope_denied`           | Token lacks the `settings` scope                                             |

## `DELETE /pipeline/{stage}/model`

Stop this stage, **keeping its backend selected and the model it was pointed
at**, so restarting it is one `POST` with no arguments to re-derive. No-op when
the stage is not running. Anything being spoken is stopped first: this is what a
Stop button calls, and a Stop that left the audio playing would not be one.

The stage reads back as `enabled: false` with `model` unchanged and
`loaded: false`. This is what frees device memory without costing the user
their choice — erasing the selection here was the old behavior, and it emptied
the settings card every time someone unloaded a model to reclaim VRAM.

Forgetting the model as well is
[`DELETE /pipeline/{stage}`](./stage.md#delete-pipelinestage), which drops the
backend with it.

**Request:**

```http
DELETE /pipeline/1/model HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
```

**Response (200):**

```json
{ "status": "success", "message": "Unloaded kokoro-82m" }
```

**Errors:** `404 unknown_stage`, plus the auth errors above.

## `POST /pipeline/{stage}/model/cancel`

Abandon the load **this stage** has in flight, including the file work feeding
it — both the `verifying` pass over what is already on disk and any download it
starts. Typically a large local model whose files the user does not want to wait
for.

Scoped to the stage that asked: a stage with nothing of its own in flight
answers `409 no_switch_in_progress` even while another stage is downloading —
that load is not this one's to abandon.

There is a window past which a switch can no longer be interrupted: once the
files are down and the weights are going onto the device, cancelling would
leave a half-instantiated backend, so the call answers `409
switch_finalizing` and the switch runs to completion. Wait for it, or start a
different switch afterwards.

**Request:**

```http
POST /pipeline/1/model/cancel HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
```

No request body.

**Response (200):**

```json
{ "status": "success", "message": "Model switch cancelled" }
```

After a successful cancel the next
[`GET /pipeline/{stage}/model`](#get-pipelinestagemodel) reports the previous
model as `model` and the abandoned load as `switch: { "phase": "cancelled", … }`,
cleared on the next switch attempt.

**Errors:**

| HTTP | `error_code`            | Meaning                                                       |
|------|-------------------------|---------------------------------------------------------------|
| 404  | `unknown_stage`         | No such position in the pipeline                              |
| 409  | `no_switch_in_progress` | This stage has nothing in flight to cancel                    |
| 409  | `switch_finalizing`     | The switch has passed the cancellable window                   |
| 401  | `invalid_session`       | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`          | Token lacks the `settings` scope                              |

## `POST /pipeline/{stage}/model/reload`

Re-instantiate this stage's model **in place** — same `(model, source)`, same
device preference — so a changed
[secret or option](../backends/options.md) takes effect without picking a
different model. A stage with nothing loaded answers `200` and does nothing.

Unlike [`POST /pipeline/{stage}/model`](#post-pipelinestagemodel), which starts
a possibly-long switch and returns before it completes, reload is
**synchronous**: the
response is sent after the model has been re-instantiated. On success the
daemon broadcasts `model_switched` then `ready` on
[`/events?topics=daemon_status_changed`](../events.md), identical to a
completed switch, so subscribers converge on the same state either way.

Rarely needed by hand: writing a
[backend option](../backends/options.md) or
[secret](../backends/secrets.md) already reloads every stage running a model
from that backend, so the new value takes effect immediately.

Anything the model is saying is stopped first, since the instance saying it is
the one being torn down — including when the reload comes from a
[secret](../backends/secrets.md) write rather than from this endpoint.

**Request:**

```http
POST /pipeline/1/model/reload HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
```

No request body.

**Response (200) — reloaded:**

```json
{ "status": "success", "message": "Successfully switched to model: kokoro-82m" }
```

**Response (200) — nothing loaded:**

```json
{ "status": "success", "message": "No active model to reload" }
```

**Errors:**

| HTTP | `error_code`         | Meaning                                                                  |
|------|----------------------|--------------------------------------------------------------------------|
| 404  | `unknown_stage`      | No such position in the pipeline                                         |
| 401  | `invalid_session`    | Token unknown / expired / `exe_changed`                                  |
| 403  | `scope_denied`       | Token lacks the `settings` scope                                         |
| 500  | *(uncoded)*          | Re-instantiation failed (`Model reload failed: …`); the previous instance is gone and the stage is left with nothing loaded |
