# Backend Configuration

Every backend ships a `backend.toml` at the root of its directory. The
configuration is the backend's declaration of identity: which models it provides,
which servers it connects to, which files those models need, and which
secrets it requires. The daemon reads it to **discover** a backend without
starting it — discovery is a filesystem scan, so installing a backend costs
nothing until a model it provides is selected.

This document is part of the [backend protocol](./contract.md); see also
[wasm.md](./wasm.md) and [subprocess.md](./subprocess.md) for how the
configuration's fields are honored per transport.

A JSON Schema for this file is generated from the canonical manifest types in
`super-stt-registry-types` and committed at `schemas/backend.schema.json`.
Backends in other repositories can reference it directly at
`https://raw.githubusercontent.com/jorge-menjivar/super-stt/main/schemas/backend.schema.json`.
Add that URL (or a relative path) as a `#:schema` comment line at the top of a
`backend.toml` to get autocomplete and validation in taplo-based editors.

## Discovery

A backend is a directory whose root contains `backend.toml`. The daemon scans
its configured backend search paths and treats every such directory as a
backend. Representative layout:

```text
<backend-dir>/
├── backend.toml            # this configuration
├── whisper-backend         # entrypoint (subprocess binary) …
│                           # … or whisper.wasm (WASM component)
└── models/                 # populated by the daemon at load time
    └── whisper-tiny/
        ├── config.json
        ├── tokenizer.json
        └── model.safetensors
```

The daemon never writes outside a backend's own directory, and a backend
reads only from it. Files a model needs are downloaded by the daemon into
the `dest` paths declared in the configuration (see
[`[[models.files]]`](#modelsfiles)).

## `[backend]`

Backend identity and packaging.

```toml
[backend]
source     = "github.com/super-stt/whisper"
name       = "Whisper (local)"
version    = "0.1.0"
kind       = "subprocess"
entrypoint = "whisper-backend"
contract   = "v1"
```

| Field        | Type   | Required | Notes                                                                 |
|--------------|--------|----------|-----------------------------------------------------------------------|
| `source`     | string | yes      | Canonical repository id for this backend. Becomes the `source` of every model it provides (see [identity](./contract.md#model-identity)). Must be unique across installed backends. |
| `name`       | string | yes      | Human-readable display name.                                          |
| `version`    | string | yes      | Backend version (semver).                                            |
| `kind`       | string | yes      | `subprocess` or `wasm` — selects the transport.                       |
| `entrypoint` | string | yes      | Path, relative to the backend directory, to the executable (`subprocess`) or the `.wasm` component (`wasm`). |
| `contract`   | string | yes      | The contract version the backend implements. Must be `v1`; unknown versions are rejected. |

## `[network]`

Outbound network the backend is permitted to reach.

```toml
[network]
allowed_hosts = ["api.openai.com"]
```

| Field           | Type             | Required | Notes                                                              |
|-----------------|------------------|----------|--------------------------------------------------------------------|
| `allowed_hosts` | array of string  | no       | Host or `host:port` egress allowlist. Empty or absent ⇒ no network. |

`allowed_hosts` is honored for `wasm` backends, where the daemon enforces it
on every outbound request (see [wasm.md](./wasm.md#network-egress)).
`subprocess` backends run with no network regardless; the field must be
empty for them.

## `[[secrets]]`

Encrypted credentials the backend needs at runtime, such as API keys. A
backend may declare several — a primary and a fallback key, or keys for
different upstreams — and use whichever it needs. The user supplies each
value through the settings UI; the daemon stores it encrypted in the system
keyring and never writes it to disk in plaintext.

```toml
[[secrets]]
name        = "openai_api_key"
label       = "OpenAI API key"
description = "Used to authenticate requests to api.openai.com."

[[secrets]]
name        = "azure_openai_key"
label       = "Azure OpenAI key"
description = "Used only when an Azure deployment is set."
required    = false
```

| Field         | Type   | Required | Notes                                                       |
|---------------|--------|----------|-------------------------------------------------------------|
| `name`        | string | yes      | snake_case identifier the backend reads the value by. `[a-z][a-z0-9_]*`, unique within the table. |
| `label`       | string | no       | Human-readable label shown in the settings UI. Falls back to `name` when absent. |
| `description` | string | yes      | Help text shown beside the input in the settings UI.        |
| `required`    | bool   | no       | Whether a value must be set before the backend can load. Default `false`. |

## `[[options]]`

Non-secret configuration the user can set through the settings UI — a
base-URL override, a timeout, and so on. Options are declared like secrets
and shown beside them, but the daemon stores their values as plaintext
configuration rather than encrypting them.

```toml
[[options]]
name        = "base_url"
label       = "API base URL"
description = "Override the API base URL, e.g. for a gateway."
type        = "string"
default     = "https://api.openai.com"

[[options]]
name        = "request_timeout_seconds"
label       = "Request timeout"
description = "Per-request timeout in seconds."
type        = "integer"
default     = 30
```

| Field         | Type           | Required | Notes                                                  |
|---------------|----------------|----------|--------------------------------------------------------|
| `name`        | string         | yes      | snake_case identifier the backend reads the value by. `[a-z][a-z0-9_]*`, unique within the table. |
| `label`       | string         | no       | Human-readable label shown in the settings UI. Falls back to `name` when absent. |
| `description` | string         | yes      | Help text shown beside the input in the settings UI.   |
| `type`        | string         | no       | `string`, `integer`, or `bool`. Drives the input the UI renders. Default `string`. |
| `default`     | matches `type` | no       | Value used when the user sets none.                    |
| `required`    | bool           | no       | Whether a value must be set before the backend can load. Default `false`. |

Both secrets and options reach the backend the same way — injected request
headers on every `/v1` request — and differ only in how the daemon stores
them at rest. See [request headers](./contract.md#request-headers).

## `[assets]`

Declares the binary artifacts a release publishes, so the registry indexer and
the daemon's installer can find them without guessing. The shape depends on the
backend's `kind`.

`[assets]` is required for registry publication (the indexer rejects a release
without the table matching the backend's `kind`) and optional for backends
installed locally — an imported directory has no release artifacts to declare.

Wasm backends declare a single file:

```toml
[assets]
wasm = "openai.wasm"   # filename on the release; must be a `wasm32` component
```

Subprocess backends declare one entry per built variant. Selection axes are
`target`, `accel`, and (when `accel = "cuda"`) `cuda_major`, `cuda_sm`, `cudnn`.

```toml
[[assets.subprocess]]
file   = "voxtral-x86_64-unknown-linux-gnu-cpu.tar.gz"
target = "x86_64-unknown-linux-gnu"
accel  = "cpu"

[[assets.subprocess]]
file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-sm75.tar.gz"
target     = "x86_64-unknown-linux-gnu"
accel      = "cuda"
cuda_major = 12
cuda_sm    = 75

[[assets.subprocess]]
file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-cudnn-sm75.tar.gz"
target     = "x86_64-unknown-linux-gnu"
accel      = "cuda"
cuda_major = 12
cuda_sm    = 75
cudnn      = true
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `file` | string | yes | Filename on the GitHub release. Subprocess: `.tar.gz`; wasm: `.wasm`. |
| `target` | string | yes | Rust target triple. Tier-1/2 only; indexer rejects unknown. |
| `accel` | string | yes | One of `"cpu"`, `"cuda"`, `"metal"`, `"rocm"`, `"vulkan"`. |
| `cuda_major` | integer | for `accel = "cuda"` | CUDA major version this build targets. |
| `cuda_sm` | integer | no | Compute capability (e.g. `75`, `86`, `90`). Omit to match **any** compute capability — use this for framework builds (e.g. a PyTorch wheel) whose kernels are multi-architecture. When both an exact-SM and a wildcard asset match a host, the exact-SM asset is preferred. |
| `cudnn` | bool | no | Defaults `false`. Allowed only when `accel = "cuda"`. |

### Subprocess archive contents

A subprocess `.tar.gz` MUST contain `bin/<entrypoint>` (the path that the
backend's `[backend].entrypoint` resolves to after extraction). Tarballs
containing path-traversal entries (`..`, absolute paths) or symlinks that
escape the archive root are rejected by the registry indexer and by the
daemon's installer.

## `[capabilities]`

Optional feature flags that unlock transport extensions beyond the base `/v1`
contract. All fields default to `false` and may be omitted entirely.

```toml
[capabilities]
websocket = true
```

| Field       | Type | Required | Notes                                                                              |
|-------------|------|----------|------------------------------------------------------------------------------------|
| `websocket` | bool | no       | Opt into the `super-stt:realtime/ws` import and the `super-stt:realtime/ws-server` export (see [wasm.md — Realtime](./wasm.md#realtime-websocket)). When `true`, the daemon wires those interfaces into the WASM component for every session on a realtime model. **wasm-only** — a `subprocess` backend declaring `websocket = true` is rejected at discovery. Default `false`. |

## `[[models]]`

One entry per model the backend provides. Each model is identified on the
wire by `(name, provider, source)`, where `source` is the `[backend].source`
above.

```toml
[[models]]
name                   = "whisper-tiny"
provider               = "local_whisper"
multilingual           = true
primary_language       = "en"
supported_languages    = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices      = ["cpu", "cuda"]
estimated_vram_bytes   = 262144000
processing_interval_ms = 1000
```

| Field                    | Type            | Required | Notes                                                            |
|--------------------------|-----------------|----------|------------------------------------------------------------------|
| `name`                   | string          | yes      | Wire model name.                                                 |
| `provider`               | string          | yes      | `local_whisper`, `local_voxtral`, `local_qwen3_asr`, `openai`, `mistral`, or `deepgram`. |
| `multilingual`           | bool            | no       | Whether the model accepts more than one language. Default `true`. When `true`, `POST /v1/transcribe` accepts a `language` from `supported_languages`. |
| `primary_language`       | string          | yes      | Default language code (e.g. `en`); used when `language` is omitted. |
| `supported_languages`    | array of string | yes      | Language codes the model accepts; must include `primary_language`. When `multilingual` is `false`, it is exactly `[primary_language]`. |
| `supported_devices`      | array of string | yes      | Devices the model can be loaded onto. Snake_case values from `["cpu", "cuda", "metal", "none"]`. `"none"` is the sentinel for remote/online models and must be the only entry when present. Non-empty. |
| `estimated_vram_bytes`   | integer         | no       | Conservative GPU memory estimate. Default `0`; use `0` for cloud models. |
| `processing_interval_ms` | integer         | no       | Suggested minimum interval between streaming passes, in ms.      |
| `realtime`               | bool            | no       | When `true`, the model is driven over the consumer-facing WebSocket endpoint (`GET /v1/transcribe/realtime`) rather than batch `POST /v1/transcribe`. Requires `[capabilities] websocket = true`. Default `false`. |

`multilingual`, `primary_language`, and `supported_languages` together
describe language capability. When `multilingual` is `true`,
`POST /v1/transcribe` may carry a `language`, which must be one of
`supported_languages`; when omitted, `primary_language` is used. When
`multilingual` is `false`, the model transcribes only `primary_language`.

`supported_devices` declares which physical/virtual devices the model can be
loaded onto. The settings app uses it to present the device choice at load
time. `"none"` marks a remote/online model with no local compute; mixing
`"none"` with any local device (`"cpu"` / `"cuda"` / `"metal"`) is a
contradiction and the manifest is rejected.

### `[[models.files]]`

Files a model needs, and where to place them. Each entry is one download
group. The daemon fetches the files into `dest` (relative to the backend
directory) before calling `POST /v1/load`. Cloud models declare no files.

```toml
[[models.files]]
source   = "huggingface"
repo     = "openai/whisper-tiny"
revision = "main"
files    = ["config.json", "tokenizer.json", "model.safetensors"]
dest     = "models/whisper-tiny"
```

| Field      | Type            | Required | Notes                                                              |
|------------|-----------------|----------|--------------------------------------------------------------------|
| `source`   | string          | no       | `huggingface` (default) or `url`.                                  |
| `repo`     | string          | for `huggingface` | Hugging Face repo id, e.g. `openai/whisper-tiny`.         |
| `revision` | string          | no       | Hugging Face revision; default `main`.                             |
| `url`      | string          | for `url` | Direct download URL for a single file.                            |
| `files`    | array of string | for `huggingface` | Filenames to fetch from the repo.                         |
| `dest`     | string          | yes      | Directory, relative to the backend dir, to place the files in.     |
| `sha256`   | string          | no       | Expected SHA-256 for integrity verification.                       |

## Example: local backend (subprocess)

A Whisper backend providing two models, loaded from Hugging Face. Whisper
models ship `config.json`, `tokenizer.json`, and a single
`model.safetensors`.

```toml
[backend]
source     = "github.com/super-stt/whisper"
name       = "Whisper (local)"
version    = "0.1.0"
kind       = "subprocess"
entrypoint = "whisper-backend"
contract   = "v1"

[network]
allowed_hosts = []

[[models]]
name                   = "whisper-tiny"
provider               = "local_whisper"
multilingual           = true
primary_language       = "en"
supported_languages    = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices      = ["cpu", "cuda"]
estimated_vram_bytes   = 262144000
processing_interval_ms = 1000

[[models.files]]
source   = "huggingface"
repo     = "openai/whisper-tiny"
revision = "main"
files    = ["config.json", "tokenizer.json", "model.safetensors"]
dest     = "models/whisper-tiny"

[[models]]
name                   = "voxtral-mini"
provider               = "local_voxtral"
multilingual           = true
primary_language       = "en"
supported_languages    = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices      = ["cuda"]
estimated_vram_bytes   = 8589934592
processing_interval_ms = 2000

# Voxtral ships tekken.json instead of tokenizer.json, and multi-shard
# weights.
[[models.files]]
source   = "huggingface"
repo     = "mistralai/Voxtral-Mini-3B-2507"
revision = "main"
files    = [
    "config.json",
    "tekken.json",
    "model-00001-of-00002.safetensors",
    "model-00002-of-00002.safetensors",
]
dest     = "models/voxtral-mini"
```

## Example: cloud backend (WASM)

An OpenAI backend. No model files; one egress host; one secret and one option.

```toml
[backend]
source     = "github.com/super-stt/openai"
name       = "OpenAI"
version    = "0.1.0"
kind       = "wasm"
entrypoint = "openai.wasm"
contract   = "v1"

[network]
allowed_hosts = ["api.openai.com"]

[[secrets]]
name        = "openai_api_key"
label       = "OpenAI API key"
description = "Used to authenticate requests to api.openai.com."

[[options]]
name        = "base_url"
label       = "API base URL"
description = "Override the API base URL, e.g. for a gateway."
type        = "string"
default     = "https://api.openai.com"

[[models]]
name                = "whisper-1"
provider            = "openai"
multilingual        = true
primary_language    = "en"
supported_languages = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices   = ["none"]

[[models]]
name                = "gpt-4o-transcribe"
provider            = "openai"
multilingual        = true
primary_language    = "en"
supported_languages = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices   = ["none"]
```

## Validation

- String-valued enums (`kind`, `provider`, file `source`, option `type`) are
  **snake_case**; unknown values are rejected and a backend whose
  configuration fails validation is skipped during discovery rather than
  loaded with defaults.
- Secret and option `name`s are **snake_case** identifiers matching
  `[a-z][a-z0-9_]*` (e.g. `openai_api_key`, `base_url`), unique within their
  table. The `name` is the wire identifier the backend reads the value by;
  `label` is the human-readable text shown beside the input in the settings
  UI. Secret values are stored encrypted; option values are stored as
  plaintext.
- `[backend].source` must be unique across installed backends; a collision
  is a discovery error for the later backend.
- A `subprocess` backend with a non-empty `allowed_hosts` is rejected — the
  transport provides no network.
- `primary_language` must appear in `supported_languages`. When
  `multilingual` is `false`, `supported_languages` must be exactly
  `[primary_language]`.
- `supported_devices` is required and non-empty for every model. Each entry
  must be one of `cpu`, `cuda`, `metal`, `none`; the sentinel `none` (remote
  / online model) must be the only entry when present. A backend whose
  manifest violates any of these is skipped during discovery.
- A `subprocess` backend that declares `[capabilities] websocket = true` is
  rejected at discovery — realtime WebSocket support is wasm-only.
- Any model entry with `realtime = true` in a backend whose
  `[capabilities] websocket` is `false` or absent is rejected at discovery.
