# `GET /gpu_info`

A read-only inventory of the GPUs visible to the daemon and how much memory
each has. Powered by driver-level queries only (NVIDIA NVML, Linux DRM sysfs,
macOS `system_profiler`/`sysctl`) — no CUDA toolkit or vendor SDK is involved,
and the call never mutates daemon state.

This is hardware discovery, distinct from a model's
[device](./pipeline/device.md#get-pipelinestagemodelmodeldevice), which selects the
*compute device* (`cpu`/`gpu`) that model loads on. The result
is a point-in-time snapshot: `total_bytes` is effectively static, but
`free_bytes`/`used_bytes` reflect the moment of the call. The daemon re-probes
on every request, so a client may poll this endpoint for a live memory view —
for example, weighing a model's `estimated_vram_bytes` from
[`/backends`](./backends.md) against `free_bytes` before a GPU load.

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
      "used_bytes":  15271460864,
      "arch_target": "sm_86"
    }
  ],
  // Host-wide toolchain/driver versions, independent of any one GPU. Each
  // entry is `null` when that accelerator isn't detected on this host.
  "host": {
    "cuda":   { "driver_version": "13.3" },
    "rocm":   null,
    "vulkan": { "api_version": "1.3.280" }
  }
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
| `arch_target` | string? | The architecture a prebuilt asset must target to run on this GPU, in the vendor's own spelling: `"sm_86"` on NVIDIA, `"gfx1030"` on AMD. `null` when the driver reports none (an Apple or Intel GPU, or an AMD card on a kernel without KFD). |
| `host`        | object  | `{ cuda, rocm, vulkan }`, each `null` or an object — see below. Describes the host's installed toolchains, not any one GPU. |
| `host.cuda`   | object? | `{ "driver_version": "13.3" }` — the installed NVIDIA driver's CUDA version. `null` when NVML is unavailable. |
| `host.rocm`   | object? | `{ "version": "6.2.4" }` — the installed `ROCm` userspace release. `null` when no `ROCm` install is found at the conventional prefixes. |
| `host.vulkan` | object? | `{ "api_version": "1.3.280" }` — the highest Vulkan API version any installed driver advertises. `null` when no Vulkan runtime is found. |

A `null` `host.rocm` does **not** mean a GPU is unusable for `ROCm` compute:
the kernel side (`amdgpu`/KFD, which is what publishes each GPU's
`arch_target`) is independent of the userspace install this field reports,
and a distro packaging `ROCm` into `/usr`, or a subprocess backend bundling
its own runtime, both report `null` here while working normally. `arch_target`
is what a build must match; `host.rocm` is advisory only.

A non-null `host.vulkan` likewise does **not** mean this host has usable GPU
compute: it reports whether a Vulkan *loader* is installed, and Mesa's
lavapipe — a software rasterizer shipped by default on many distributions —
is a loader like any other, so a machine with no GPU at all can still report
a `host.vulkan` version here. `available_devices` on a model's
[device](./pipeline/device.md#get-pipelinestagemodelmodeldevice) is the
authoritative capability answer; `host.vulkan` is advisory only, the same as
`host.rocm` above.

**Errors:**

| HTTP | `message`         | Meaning                                  |
|------|-------------------|------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |
