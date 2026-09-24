# Subprocess Backends

A subprocess backend is a native executable the daemon spawns and talks to
over a Unix socket. Use this transport for **local models and GPU
inference**: native code keeps full access to CUDA, Metal, and the host's
ML stack, with no WASM or `wasi-nn` constraints.

This document is part of the [backend protocol](./contract.md). The contract
itself — the `/v1` routes, payloads, and lifecycle — is defined there; this
document covers only what is specific to the subprocess transport. Configuration
fields are described in [config.md](./config.md).

A subprocess backend declares `kind = "subprocess"` and an `entrypoint`
pointing at its executable.

## Transport

The backend is an HTTP/1.1 server that exposes the [`/v1`
routes](./contract.md#the-v1-contract), including the SSE form of
`POST /v1/transcribe`. The daemon is the client. This is the same wire shape
as the external client↔daemon protocol in [transport.md](../transport.md),
so an existing HTTP/SSE server stack can be reused directly.

The daemon provides the socket; the backend binds and serves it. The socket
**must be a pathname socket** (a path on disk), not an abstract-namespace
socket. Abstract sockets are scoped to a network namespace, and the sandbox
places the backend in its own namespace (see [Sandbox](#sandbox)) — a
pathname socket is a filesystem object and keeps working across that
boundary, while an abstract socket would not.

## Startup

The daemon spawns the executable with its working directory set to the
backend directory and the following environment:

| Variable                   | Notes                                                                 |
|----------------------------|-----------------------------------------------------------------------|
| `SUPER_STT_BACKEND_SOCKET` | Pathname of the Unix socket to bind and serve the `/v1` routes on.    |
| `SUPER_STT_BACKEND_DIR`    | Absolute path to the backend directory; model files live under it at the configured `dest` paths. |
| `SUPER_STT_BACKEND_CACHE_DIR` | Absolute path to a writable directory the backend may keep regenerable data in, private to it and preserved across runs. The daemon creates it; it is the **only** durable writable path the sandbox grants. |
| `XDG_CACHE_HOME`           | Set to the same directory, so a library that resolves its own cache the XDG way lands there instead of under the read-only `$HOME`. |
| `CUDA_CACHE_PATH`          | Set to `<cache dir>/nv`. The NVIDIA driver keeps its PTX-to-SASS translations under `$HOME/.nv/ComputeCache` and ignores XDG, so it is the one consumer the line above misses — and `ProtectHome=read-only` would let it read a cache it can never write, redoing the translation on every load. |
| `CUDA_CACHE_MAXSIZE`       | Bounds that cache. The driver's own default is around a gigabyte, and the cache is per backend, so the default would be inherited once per installed backend. |

On startup the backend binds `SUPER_STT_BACKEND_SOCKET`, begins serving
`/v1`, and reports `state: "starting"` from `GET /v1/status` until a
`POST /v1/load` arrives. It resolves a model's files from
`SUPER_STT_BACKEND_DIR` joined with the model's `dest`.

The daemon polls for `ready` for **ten minutes** before giving up on a load,
or for as long as the load keeps moving if the backend reports its progress
(see [`GET /v1/status`](./contract.md#get-v1status)). Ten minutes is a
generous budget on purpose: a backend that compiles its GPU kernels
at runtime should do it during the load, not on the first request, because
`ready` is what the daemon shows the user and what it starts sending
transcription requests against. A load that fails must say so with `error`
rather than leave the daemon waiting out the budget (see
[`GET /v1/status`](./contract.md#get-v1status)).

Anything a backend wants to keep between runs goes in
`SUPER_STT_BACKEND_CACHE_DIR` and must be treated as regenerable: it is a
cache, the daemon does not back it up, and a user may delete it. A backend
that compiles GPU kernels at runtime — anything on CubeCL/Burn — should point
its kernel cache there, or it recompiles on every load. A backend with
nothing to keep can ignore the variable.

Secrets and options are not passed through the environment; the daemon
injects them as request headers on each `/v1` request (see
[request headers](./contract.md#request-headers)). A network-isolated
local backend usually declares none.

The daemon stops a backend by sending `SIGTERM`, then `SIGKILL` if it does
not exit promptly. Backends should handle `SIGTERM` by closing the socket
and exiting. Because the daemon **terminates the active backend before
spawning the next** (see [lifecycle](./contract.md#lifecycle)), a backend
process serves exactly one loaded model over its lifetime.

## Sandbox

The daemon runs each subprocess backend in a hardened, transient systemd
unit. The backend cannot relax these restrictions; design against them:

| Restriction                       | Effect on the backend                                          |
|-----------------------------------|----------------------------------------------------------------|
| `PrivateNetwork=yes`              | No IP network of any kind. The Unix socket still works.        |
| `ProtectSystem=strict`            | The entire filesystem is read-only …                           |
| `ReadOnlyPaths=<backend dir>`     | … including the backend's own directory: the daemon provisions model files before the unit spawns, and the backend never writes there. |
| `ReadWritePaths=<socket dir>`     | The socket directory is writable.                              |
| `ReadWritePaths=<cache dir>`      | `SUPER_STT_BACKEND_CACHE_DIR` is writable, and is the only writable path whose contents survive the process. Every cache the daemon points a library at lives under it, including the driver's. |
| `ProtectHome=read-only`, `PrivateTmp=yes` | `$HOME` is readable but not writable; `/tmp` is private and writable, but it is discarded with the unit — scratch only, never a cache. |
| `NoNewPrivileges=yes`             | The process cannot acquire new privileges.                     |
| `SystemCallFilter=@system-service` | A seccomp allowlist; privileged syscall groups are denied.    |
| `PrivateDevices=yes`              | A private `/dev` with no GPU nodes, unless the model declares a GPU. |

Two consequences worth stating plainly:

- **No network.** A subprocess backend can never reach the internet. Any
  file a model needs is downloaded by the daemon (which has network) into
  the backend directory before `load`. This is why `allowed_hosts` must be
  empty for subprocess backends.
- **GPU access is a deliberate hole.** Computing on an accelerator requires
  exposing the GPU device nodes, and the GPU driver is privileged kernel
  attack surface. This is inherent to GPU compute and is not closed by the
  sandbox; it is the reason untrusted, network-facing backends belong on the
  WASM transport instead. The hole is opened only where it is needed, and the
  manifest decides: a model whose [`supported_devices`](./config.md#models)
  names `gpu` is spawned with the GPU nodes bound in — NVIDIA's
  `/dev/nvidia0`, `/dev/nvidiactl`, `/dev/nvidia-uvm` and
  `/dev/nvidia-uvm-tools`, AMD's `/dev/kfd`, and every DRM render node
  (`/dev/dri/renderD*`) present on the host, since the render minor a given
  card answers on is not knowable in advance. A model that does not name
  `gpu` — CPU-only, or the `none` sentinel of a remote model — runs with a
  private `/dev` holding only the pseudo-devices, so the GPU nodes are not
  there to open. The sandbox is fixed when the unit spawns, which is before
  `load`, so the `device` a load request asks for cannot widen it; declare
  every device the model can use.

A subprocess backend whose CUDA kernels are multi-architecture (for example a
bundled PyTorch wheel) may publish a single CUDA asset that omits `cuda_sm`;
the daemon then matches it against any GPU compute capability whose runtime
major is `>=` the asset's `cuda_major`. Backends that AOT-compile per
architecture (e.g. candle) keep one asset per `cuda_sm`; an exact-SM asset is
preferred over a wildcard when both match.

## Authentication

The daemon creates the socket in a directory it owns, with permissions that
admit only the daemon, and verifies the peer with `SO_PEERCRED`. The
daemon↔backend channel therefore carries **no bearer tokens and no consent
flow** — unlike external clients. A backend does not implement
authentication; it serves whatever connects on its socket. It may verify via
`SO_PEERCRED` that the peer is the daemon, but this is optional.

## Implementation checklist

- Declare `kind = "subprocess"` and an `entrypoint` in
  [backend.toml](./config.md).
- Bind `SUPER_STT_BACKEND_SOCKET` (a pathname socket) and serve the
  [`/v1` routes](./contract.md#the-v1-contract).
- Resolve model files under `SUPER_STT_BACKEND_DIR`; never attempt network
  access.
- Drive `GET /v1/status` through `starting → loading → ready`, reporting load
  `progress` and the actual `device`.
- Stream `event: preview` / `event: done` from `POST /v1/transcribe` when
  `options.stream_realtime` is set.
- Exit cleanly on `SIGTERM`.
