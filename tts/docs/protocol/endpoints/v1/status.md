# `GET /status`

Snapshot of the daemon's current operational state — which model is
loaded and which device it's running on. Subscriber introspection
and other operator info are not exposed here; for those, the
`settings` scope's [`GET /pipeline/{stage}/model`](./pipeline/model.md#get-pipelinestagemodel)
and [`GET /pipeline/{stage}/model/{model}/device`](./pipeline/device.md#get-pipelinestagemodelmodeldevice)
endpoints apply.

## Auth

- **Required scope:** `status`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `status` scope get `403 scope_denied`.

## `GET /status`

**Request:**

```http
GET /status HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":        "success",
  "device":        "cuda",
  "model_loaded":  true,
  "current_model": "kokoro-82m",
  "busy":          false
}
```

| Field           | Type    | Notes                                                                                  |
|-----------------|---------|----------------------------------------------------------------------------------------|
| `device`        | string  | The accelerator the loaded model is actually running on: `"cpu"`, `"cuda"`, `"rocm"`, `"metal"`, `"vulkan"`, or `"remote"` for an online model. `"unknown"` if nothing is loaded. |
| `model_loaded`  | bool    | `false` while the daemon is still loading the initial model or after a failed switch   |
| `current_model` | string? | The loaded model's name (e.g. `kokoro-82m`); absent when `model_loaded` is `false`   |
| `busy`          | bool    | `true` while an utterance is being synthesized or played out. Clients implementing a speak/silence toggle consult this and call [`POST /speak/stop`](./speak/stop.md) when `true`, [`POST /speak`](./speak.md) when `false`. Note that `/speak` itself is *not* gated on it — a new utterance deliberately interrupts the current one. |

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed` — re-auth and retry   |
| 403  | `scope_denied`    | Token lacks the `status` scope                               |
