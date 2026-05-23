# `/active_device`

Read and switch the device (CPU vs GPU) the active model runs on.
Setting a new device triggers a background reload of the current
model; watch the [`daemon_status_changed`](./events.md) SSE topic
for the post-reload `status: "ready"` event with the new device.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- `client` / `widget` tokens get `403 scope_denied`.

## `POST /active_device`

**Request:**

```http
POST /active_device HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "device": "cuda"
}
```

| Field    | Type   | Required | Notes                              |
|----------|--------|----------|------------------------------------|
| `device` | string | yes      | One of `"cpu"`, `"cuda"`, `"metal"` |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":            "success",
  "device":            "cuda",
  "available_devices": ["cpu", "cuda"]
}
```

| Field               | Type     | Notes                                                                                   |
|---------------------|----------|-----------------------------------------------------------------------------------------|
| `device`            | string   | The new device preference                                                                |
| `available_devices` | string[] | The devices reachable on this host                                                       |

The model reload itself runs in the background — subscribers to
[`/events?topics=daemon_status_changed`](./events.md) see a
`status: "ready"` event when it completes, carrying the resolved
`actual_device` (which may be `"cpu"` if `"cuda"` was requested but
the load fell back).

**Errors:**

| HTTP | `message`             | Meaning                                                       |
|------|-----------------------|---------------------------------------------------------------|
| 400  | `cuda_unavailable`    | `device: "cuda"` requested on a host with no CUDA support      |
| 400  | `invalid_device`      | `device` wasn't one of `cpu`, `cuda`, `metal`                  |
| 401  | `invalid_session`     | Token unknown / expired / `exe_changed`                        |
| 403  | `scope_denied`        | Not a `settings` token                                         |

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
  "device":            "cuda",
  "available_devices": ["cpu", "cuda"],
  "gpu_free_memory":   8123456789,
  "gpu_total_memory":  25395560448
}
```

| Field               | Type     | Notes                                                                            |
|---------------------|----------|----------------------------------------------------------------------------------|
| `device`            | string   | The active device                                                                 |
| `available_devices` | string[] | Devices reachable on this host                                                    |
| `gpu_free_memory`   | u64?     | Free GPU memory in bytes (omitted on hosts with no GPU)                           |
| `gpu_total_memory`  | u64?     | Total GPU memory in bytes (omitted on hosts with no GPU)                          |

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Not a `settings` token                                        |
