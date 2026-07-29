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

On startup the backend binds `SUPER_STT_BACKEND_SOCKET`, begins serving
`/v1`, and reports `state: "starting"` from `GET /v1/status` until a
`POST /v1/load` arrives. It resolves a model's files from
`SUPER_STT_BACKEND_DIR` joined with the model's `dest`.

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
| `ReadWritePaths=<backend dir>`    | … except the backend's own directory.                          |
| `ProtectHome=yes`, `PrivateTmp=yes` | `$HOME` is inaccessible; `/tmp` is private.                  |
| `NoNewPrivileges=yes`             | The process cannot acquire new privileges.                     |
| `SystemCallFilter=@system-service` | A seccomp allowlist; privileged syscall groups are denied.    |
| `PrivateDevices=yes`              | A private `/dev` with no GPU nodes, unless the model declares a GPU. |

Two consequences worth stating plainly:

- **No network.** A subprocess backend can never reach the internet. Any
  file a model needs is downloaded by the daemon (which has network) into
  the backend directory before `load`. This is why `allowed_hosts` must be
  empty for subprocess backends.
- **GPU access is a deliberate hole.** Using CUDA or Metal requires exposing
  the GPU device nodes, and the GPU driver is privileged kernel attack
  surface. This is inherent to GPU compute and is not closed by the sandbox;
  it is the reason untrusted, network-facing backends belong on the WASM
  transport instead. The hole is opened only where it is needed, and the
  manifest decides: a model whose `supported_devices` names `cuda` or `metal`
  is spawned with the host `/dev`. A model that names neither — CPU-only, or
  the `none` sentinel of a remote model — runs with a private `/dev` holding
  only the pseudo-devices, so the GPU nodes are not there to open. The
  sandbox is fixed when the unit spawns, which is before `load`, so the
  `device` a load request asks for cannot widen it; declare every device the
  model can use.

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
