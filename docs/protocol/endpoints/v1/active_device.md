# `/active_device`

Read and switch the device (CPU vs GPU) the active model runs on.
When a local model is loaded, setting a new device triggers a
background reload onto it — watch the
[`daemon_status_changed`](./events.md) SSE topic for the
post-reload `status: "ready"` event with the new device. When no
model is loaded (or the loaded model is an online/cloud one), the
call only records the preference; the next local model load picks
it up, and the response returns immediately.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /active_device`

**Request:**

```http
POST /active_device HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "device": "gpu"
}
```

| Field    | Type   | Required | Notes                                                                                                    |
|----------|--------|----------|--------------------------------------------------------------------------------------------------------|
| `device` | string | yes      | `"cpu"` or `"gpu"`. `"cuda"` and `"metal"` are accepted input spellings, normalized to `"gpu"`. Any other value is rejected. |

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
| `device`            | string   | The new device preference, `"cpu"` or `"gpu"` (already normalized).                      |
| `resolved_accel`    | string?  | The accelerator `"gpu"` resolved to once a local model has loaded onto it: `"cuda"`, `"rocm"`, `"metal"`, or `"vulkan"`. `"cpu"` when the preference itself is `"cpu"` — no resolution is needed. Also `"cpu"` with `device: "gpu"` when a GPU load fell back (see the fallback note below) — the field always reports what actually loaded, not the request. `null` when the preference is `"gpu"` but no local model has loaded yet, so nothing has resolved. |
| `available_devices` | string[] | The devices reachable on this host — see [below](#get-active_device).                    |

When a local model is loaded, the reload itself runs in the
background — subscribers to
[`/events?topics=daemon_status_changed`](./events.md) see a
`status: "ready"` event when it completes, carrying the resolved
`actual_device` (which may be `"cpu"` if `"gpu"` was requested
but the load fell back). When no model is loaded the response
returns synchronously and no `ready` event is emitted — the
preference takes effect on the next model load.

**Errors:** the device-validation failure carries its machine-readable
identifier in `error_code` (see [transport.md](../../transport.md)); the standard
auth failures carry theirs in `message`, as elsewhere.

| HTTP | Identifier        | Carried in   | Meaning                                                                                    |
|------|-------------------|--------------|----------------------------------------------------------------------------------------------|
| 400  | `invalid_device`  | `error_code` | `device` wasn't `"cpu"`, `"gpu"`, or one of the accepted aliases (`"cuda"`, `"metal"`). The stored preference is left unchanged. |
| 401  | `invalid_session` | `message`    | Token unknown / expired / `exe_changed`                                                    |
| 403  | `scope_denied`    | `message`    | Token lacks the `settings` scope                                                            |

Requesting `device: "gpu"` on a host without a usable accelerator is **not**
an error — the daemon accepts it and silently falls back to CPU (surfaced by
the resolved `actual_device` in the `ready` event and the next
`GET /active_device`). It does not emit a distinct error code.

## `GET /active_device`

**Request:**

```http
GET /active_device HTTP/1.1
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

| Field               | Type     | Notes                                                                            |
|---------------------|----------|-----------------------------------------------------------------------------------|
| `device`            | string   | The active device preference, `"cpu"` or `"gpu"`.                                 |
| `resolved_accel`    | string?  | The accelerator `"gpu"` resolved to, as under `POST` above.                       |
| `available_devices` | string[] | Devices reachable on this host.                                                   |

`available_devices` is `["cpu"]`, plus `"gpu"` when the host reports a CUDA
compute capability, an AMD gfx target, or a Vulkan runtime. This endpoint
answers for the *host* — a daemon-global capability check, independent of
which model is active. A per-model list is narrower: intersect a model's
`supported_devices` with the accelerators its installed asset actually
provides, reported as `installed_accel` on [`GET /backends`](./backends.md).

GPU memory (free / total / used per device) is reported by
[`GET /gpu_info`](./gpu_info.md), not this endpoint.

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|-----------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Token lacks the `settings` scope                                |
