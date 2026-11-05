# `/tts/pipeline/{stage}/model/{model}/device`

Which framework a model runs on, and which this machine can offer it.

A device is a **framework**: `cpu`, `cuda`, `rocm`, `metal` or `vulkan`. A GPU
is reached through one of them, and which one is the choice, so there is no
`gpu`. A model's manifest still declares what it supports on the `cpu`/`gpu`
axis (`supported_devices`); `cuda`, `rocm`, `metal` and `vulkan` are its `gpu`.
The frameworks offered are every one a published build of the backend would
run on this machine, installed or not; switching to one whose build is not
installed takes an install first, with
[`POST /registry/backend/install`](../../registry/install.md) and that `accel`.
A build runs on the frameworks its [`accel`](../../../../backend/config.md#assets)
lists, so one listing `["cuda", "cpu"]` moves to the CPU at once.

The preference belongs to the **model**, not to the stage and not to the
daemon: a 82M-parameter preset-voice model runs fine on the CPU while the
cloning model beside it needs the GPU, and a user who chose one for each should
not have their choice overwritten by whichever they configured last. It is
remembered per `(source, model)`, and addressed through the
[stage](./stage.md) that runs it, because that is what resolves a bare model
name to a backend.

> **Replaces `/active_device`,** which set one device for the whole daemon.
> A model with no framework of its own falls back to the global value in
> `daemon.ron` when that is one it can run on here, and otherwise to the best
> framework whose build is installed.

Two list endpoints sit beside it: the per-model list, which is the narrow and
accurate answer once a model is chosen, and
[`/tts/pipeline/{stage}/device/list`](#get-ttspipelinestagedevicelist), the
backend-wide union for a client filling a picker before one is. The
[auth](../pipeline.md#auth) is the family's.

> [`GET /gpu_info`](../../gpu_info.md) reports the host's hardware without regard
> to what a model supports — it is the memory figure a large model's
> `estimated_vram_bytes` is weighed against, not the source of a device
> picker. Fill a picker from `available_devices` here instead.

## `GET /tts/pipeline/{stage}/model/{model}/device`

The framework `model` runs on. `model` is resolved against the backend selected
for this stage — the same resolution an omitted `source` gets on
[`POST /tts/pipeline/{stage}/model`](./model.md#post-ttspipelinestagemodel) — so the
stage must have a backend selected, or this answers `400 invalid_backend`.

**Request:**

```http
GET /tts/pipeline/1/model/kokoro-82m/device HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":            "success",
  "device":            "vulkan",
  "resolved_accel":    "vulkan",
  "available_devices": ["cuda", "vulkan", "cpu"]
}
```

| Field               | Type     | Notes                                                                                   |
|---------------------|----------|-----------------------------------------------------------------------------------------|
| `device`            | string   | The framework the model loads on: its own, when it is one offered here; otherwise the best offered framework whose build is installed. A client staging a load from this value never asks for a build that is not there. `"none"` for an online model, which synthesizes remotely and has no local device. |
| `resolved_accel`    | string?  | The framework the model is actually running on while it is loaded in this stage, so a choice that fell back reads `"cpu"` here. When the model is not loaded: `"cpu"` for a `"cpu"` preference (nothing to resolve), `null` for a GPU framework (nothing has loaded yet). `null` for an online model. |
| `available_devices` | string[] | The frameworks this machine can offer the model, best first: every framework a build of its backend would run on here (the release's `[[assets.subprocess]]`, in the installed `backend.toml`), on the axis the model declares. A backend whose manifest names no builds offers what is installed. Empty for an online model. |

**Errors:**

| HTTP | `error_code`      | Meaning                                                                    |
|------|-------------------|----------------------------------------------------------------------------|
| 404  | `unknown_stage`   | No such position in the pipeline                                           |
| 400  | `invalid_backend` | No backend is selected for this stage, so there is nothing to resolve `model` against |
| 400  | `invalid_model`   | The selected backend serves no `model`, or `model` does not carry this stage's role |
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                                    |
| 403  | `scope_denied`    | Token lacks the `settings` scope                                           |

## `POST /tts/pipeline/{stage}/model/{model}/device`

Run `model` on `device`. For the model loaded in this stage this is a reload
onto the new framework; for any other it only records the choice, which the
model's next load picks up. Either way the choice is the model's own from then
on: it is remembered per `(source, model)`, so two models on one backend can
run on different frameworks.

**Request:**

```http
POST /tts/pipeline/1/model/kokoro-82m/device HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
Content-Type: application/json

{ "device": "vulkan" }
```

| Field    | Type   | Required | Notes                                                                                                    |
|----------|--------|----------|----------------------------------------------------------------------------------------------------------|
| `device` | string | yes      | A framework: `"cpu"`, `"cuda"`, `"rocm"`, `"metal"` or `"vulkan"`. It must be one of `available_devices`, and its build must be installed. |

**Response (200):** the same body `GET` answers with, reflecting the new state.
`message` says which of the three things happened: recorded for the next load,
reloaded, or already on that device.

The reload, when there is one, completes before the response, and publishes
`switching_device`, `loading_model_for_device` and then `ready` — carrying the
resolved `actual_device` — on [`/events`](../../events.md). The stage unloads
before it loads, because a backend cannot be instantiated twice for the same
model: the running instance holds the name its replacement needs. An utterance
therefore cannot be in flight across the swap, so anything the model is saying
is stopped first.

If the reload fails, the model is put back on the device it had and its setting
is left as it was, and the call fails with the reason — the user's choice is
not recorded against a load that did not happen. Should putting it back fail
too, the stage is left with nothing loaded and publishes a `ready` saying so.

Asking for the framework a loaded model is already running on reloads nothing,
but one stored and fallen back to the CPU **is** retried, which is what
tracking the preference and the running framework separately is for: a user who
freed the VRAM a first attempt lacked gets the retry they were asking for.

**Errors:** those of `GET` above, plus:

| HTTP | `error_code`         | Meaning                                                                    |
|------|----------------------|----------------------------------------------------------------------------|
| 400  | `invalid_device`      | `device` is not a framework; or the model's manifest does not declare its axis (a CPU-only model cannot be sent to a GPU framework); or it is not one this machine can offer the model; or the model is online and has no local device. Nothing is recorded. |
| 409  | `build_not_installed` | The backend publishes a build for `device`, but the one installed does not list it. Install it with `POST /registry/backend/install` and its `accel`, then ask again. Nothing is recorded. |
| 500  | *(uncoded)*           | The reload failed. The `message` names the failure and the framework the model was put back on; the setting is unchanged. |

A load that cannot come up on the framework asked for falls back to the CPU,
surfaced as `resolved_accel: "cpu"` and the `ready` event's `actual_device`.
Synthesis on the CPU is slower but correct, so failing the call would cost the
user speech to save them a latency surprise.

## `GET /tts/pipeline/{stage}/model/{model}/device/list`

The frameworks this machine can offer `model` — the
`available_devices` of the model's [device](#get-ttspipelinestagemodelmodeldevice),
on its own, for a client that only wants to fill a picker. `model` resolves the
same way, and the same errors apply.

**Request:**

```http
GET /tts/pipeline/1/model/kokoro-82m/device/list HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":            "success",
  "available_devices": ["cuda", "vulkan", "cpu"]
}
```

Empty for an online model, and for a local model no build of its backend runs
on here. A client hides the device control on an empty list rather than
special-casing a status.

## `GET /tts/pipeline/{stage}/device/list`

The frameworks the backend selected for this stage can be run on here: the union
of the list above over the models it serves **for this stage's role**. This is
the answer for a card that has a backend but no model yet — showing a device
control that will be wrong once a model is picked is worse than showing the
union and narrowing it. Always best first: `cuda`, `rocm`, `metal`, `vulkan`,
then `cpu`.

**Request:**

```http
GET /tts/pipeline/1/device/list HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":            "success",
  "available_devices": ["cuda", "vulkan", "cpu"]
}
```

**Errors:** `404 unknown_stage`; `400 invalid_backend` when no backend is
selected for the stage; plus the auth errors above.
