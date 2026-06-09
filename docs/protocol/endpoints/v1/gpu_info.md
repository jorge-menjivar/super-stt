# `GET /gpu_info`

A read-only inventory of the GPUs visible to the daemon and how much memory
each has. Powered by driver-level queries only (NVIDIA NVML, Linux DRM sysfs,
macOS `system_profiler`/`sysctl`) — no CUDA toolkit or vendor SDK is involved,
and the call never mutates daemon state.

This is hardware discovery, distinct from [`/active_device`](./active_device.md),
which selects the *compute device* (`cpu`/`cuda`) a model loads on. The result
is a point-in-time snapshot: `total_bytes` is effectively static, but
`free_bytes`/`used_bytes` reflect the moment of the call. The daemon re-probes
on every request, so a client may poll this endpoint for a live memory view —
for example, weighing a model's `estimated_vram_bytes` from
[`/backends`](./backends.md) against `free_bytes` before a CUDA load.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `GET /gpu_info`

**Request:**

```http
GET /gpu_info HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{
  "status": "success",
  // One entry per detected GPU; an empty array when none are found or the
  // platform is unsupported.
  "gpu_info": [
    {
      "name":        "NVIDIA GeForce RTX 3090",
      "vendor":      "nvidia",
      "total_bytes": 25757220864,
      "free_bytes":  10485760000,
      "used_bytes":  15271460864
    }
  ]
}
```

| Field         | Type    | Notes                                                                                       |
|---------------|---------|---------------------------------------------------------------------------------------------|
| `gpu_info`    | array   | One object per GPU; `[]` when none are detected or the platform is unsupported              |
| `name`        | string  | Human-readable device name                                                                  |
| `vendor`      | string  | One of `nvidia`, `amd`, `intel`, `apple`, `unknown`                                         |
| `total_bytes` | integer | Dedicated VRAM for discrete GPUs; the shared system-memory ceiling for integrated/unified GPUs |
| `free_bytes`  | integer? | Free memory when the platform reports it; `null` otherwise (e.g. integrated GPUs)          |
| `used_bytes`  | integer? | Used memory when the platform reports it; `null` otherwise                                  |

**Errors:**

| HTTP | `message`         | Meaning                                  |
|------|-------------------|------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |
