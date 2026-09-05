# `/pipeline/{stage}/model`

The model one [stage](./stage.md) is pointed at: read it, run it, stop it,
abandon a load still in flight, or re-instantiate it in place.

All of them are scoped to the stage in the path. The stages provision
independently, so one stage's cancel is not a licence to abandon another's
download, and a post-processor's reload leaves the transcription model alone.

Emptying the stage entirely — forgetting the backend along with the model — is
[`DELETE /pipeline/{stage}`](./stage.md#delete-pipelinestage) instead. The
[auth](../pipeline.md#auth), the [model object](../pipeline.md#the-model-object)
and its [`device`](../pipeline.md#the-device-object) and
[`switch`](../pipeline.md#the-switch-object) sub-objects are described in the
family overview.

## `GET /pipeline/{stage}/model`

What this stage is pointed at, whether it is running, the accelerator it runs
on, and the load still in flight.

```http
GET /pipeline/1/model HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "success",
  "model": {
    "stage":  1,
    "model":  "whisper-large-v3",
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
time. `loaded` is what says whether it is up. Both stages answer this way; stage
1 collapsed the two into one bit until the stages were made to behave alike.

An empty slot reports `null` rather than omitting the keys:

```json
{ "stage": 2, "model": null, "loaded": false, "device": null, "switch": null }
```

**Errors:** `404 unknown_stage`, plus the auth errors below.

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

The model loads on its own [device](./device.md#get-pipelinestagemodelmodeldevice), which
is not part of this request: set it first, and it is remembered for every
later load of that model. Changing the device of a model that is **already
running** does not need this endpoint at all — `POST` the new device and the
daemon reloads it in place.

Stage 1 answers `202 Accepted` and switches asynchronously, reporting
progress on [`/events`](../events.md); stage 2 answers `200` once the load has
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
| 409  | `recording_in_progress`  | A recording is in flight                                             |
| 401  | `invalid_session`        | Token unknown / expired / `exe_changed`                              |
| 403  | `scope_denied`           | Token lacks the `settings` scope                                     |

## `DELETE /pipeline/{stage}/model`

Stop this stage, **keeping its backend selected and the model it was pointed
at**, so restarting it is one `POST` with no arguments to re-derive. No-op when
the stage is not running.

The stage reads back as `enabled: false` with `model` unchanged and
`loaded: false`. Every stage behaves this way; stage 1 used to erase its
selection here, because without an `enabled` flag that was the only way to stop
a restart reloading what had just been stopped.

Forgetting the model as well is
[`DELETE /pipeline/{stage}`](./stage.md#delete-pipelinestage), which drops the
backend with it.

**Errors:** `404 unknown_stage`, `409 recording_in_progress`, plus the auth
errors above.

## `POST /pipeline/{stage}/model/cancel`

Abandon the load **this stage** has in flight, including the download feeding
it. Every stage implements it: a post-processor downloads its weights like any
other model.

Scoped to the stage that asked. A stage with nothing of its own in flight
answers `409` even while another stage is downloading — that load is not this
one's to abandon.

**Errors:** `409 no_switch_in_progress` when this stage has no load in progress,
`404 unknown_stage`, plus the auth errors above.

## `POST /pipeline/{stage}/model/reload`

Re-instantiate this stage's model in place, picking up changed
[secrets and options](../backends.md) without a manual stop/start. A stage with
nothing loaded answers `200` and does nothing.

Rarely needed by hand: writing a
[backend option or secret](../backends/options.md) already reloads every stage
running a model from that backend, so the new value takes effect immediately.

**Errors:** `404 unknown_stage`, `409 recording_in_progress`, plus the auth
errors above.
