# `/pipeline/{stage}/model/{model}/device`

Which accelerator a model runs on, and what this install can offer it.

The preference belongs to the **model**, not to the stage: a small model runs
fine on the CPU while the large one beside it needs the GPU, and a
post-processor sharing the pipeline with either has its own answer again. It is
remembered per `(source, model)`, and addressed through the
[stage](./stage.md) that runs it, because that is what resolves a bare model
name to a backend.

Two list endpoints sit beside it: the per-model list, which is the narrow and
accurate answer once a model is chosen, and `/pipeline/{stage}/device/list`,
the backend-wide union for a client filling a picker before one is. The
[auth](../pipeline.md#auth) is the family's.

> `GET /gpu_info` reports the host's hardware without regard to what a model
> supports; fill a device picker from `available_devices` here instead.

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
| `available_devices` | string[] | The devices this install can offer the model on this host: the model's `supported_devices`, narrowed to the accelerators its installed asset actually provides (`installed_accel` on [`GET /backend/list`](../backend/list.md)) and to what the host has. A CUDA-only backend on a host without an NVIDIA GPU installed its CPU asset, and is not offered a GPU here. Empty for an online model. |

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
[`/events`](../events.md), reloads, and publishes `ready` with the resolved
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
