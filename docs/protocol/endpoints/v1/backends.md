# `/backends`

Inspect installed backends and configure their **secrets** and **options**.
Each backend is
discovered from a `backend.toml` on disk (see
[`docs/protocol/backend/config.md`](../../backend/config.md)) and declares the
models it serves plus the **secrets** and **options** it accepts.

This endpoint drives the settings UI's per-backend configuration section. The
flat model picker lives at [`GET /models`](./models.md); switching the active
model is [`POST /active_model`](./active_model.md).

## Secrets vs. options

A backend declares two kinds of user-provided configuration, each managed
through its own sub-resource. The daemon owns storage for both — clients
configure them only through these endpoints, never by touching storage
directly.

- **Secrets** (`[[secrets]]`) — sensitive values such as API keys, managed
  under [`/backends/{source}/secrets`](./backends/secrets.md) (the `secrets`
  scope). The daemon stores them in the **system keyring** and reads them only
  at model-load time, injecting each as an `x-stt-secret-<name>` request header
  (see [contract.md](../../backend/contract.md#request-headers)). Values are
  **write-only**: a client sets or clears a secret and can check whether one is
  configured, but no endpoint ever returns a value.
- **Options** (`[[options]]`) — non-sensitive configuration such as a base URL,
  managed under [`/backends/{source}/options`](./backends/options.md) (the
  `settings` scope). The daemon stores them as plaintext in its config, and
  their values *are* returned.

The keyring account for a backend secret is `backend:<source>:<name>` under the
`super-stt` service, where `<source>` is the backend's repo id.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `GET /backends`

List every installed backend with the models it serves and its declared
secrets and options.

**Request:**

```http
GET /backends HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{
  "status": "success",
  "backends": [
    {
      "source": "github.com/super-stt/openai",
      "name":   "OpenAI",
      "kind":   "wasm",                 // "wasm" | "subprocess"
      "allowed_hosts": ["api.openai.com"],  // hosts the backend may reach; [] for subprocess/local
      "models": [
        {
          "name":                 "whisper-1",
          "provider":             "openai",
          "multilingual":         true,
          "primary_language":     "en",           // model's default language (BCP-47 tag)
          "supported_languages":  ["en", "es-419", "es-ES", "fr"],  // accepted BCP-47 tags
          "supported_devices":    ["none"],
          "estimated_vram_bytes": 0           // conservative GPU estimate; 0 = cloud/unknown
        }
      ],
      "secrets": [
        {
          "name":        "openai_api_key",
          "label":       "OpenAI API key",
          "description": "Used to authenticate requests to api.openai.com.",
          "required":    true
        }
      ],
      "options": [
        {
          "name":        "base_url",
          "label":       "API base URL",
          "description": "Override the API base URL, e.g. for a gateway.",
          "type":        "string",
          "default":     "https://api.openai.com",
          "required":    false,
          "value":       "https://api.openai.com"  // effective value (override or default)
        }
      ]
    }
  ]
}
```

| Field             | Type             | Notes                                                                 |
|-------------------|------------------|-----------------------------------------------------------------------|
| `backends`        | array of objects | One per installed backend.                                            |
| `…[].source`      | string           | Backend repo id; the `source` of every model it serves.              |
| `…[].name`        | string           | Human-readable backend name.                                         |
| `…[].kind`        | string           | `wasm` or `subprocess`.                                              |
| `…[].allowed_hosts` | array of strings | Hosts the backend is permitted to reach (`[network].allowed_hosts` from its `backend.toml`). Empty for `subprocess` backends (which run with no network) and for backends that declare none. Surfaced in the settings UI's "Online model" badge so the user sees where a cloud backend's audio would go. |
| `…[].models`      | array            | Models served, as `{ name, provider, multilingual, primary_language, supported_languages, supported_devices, estimated_vram_bytes }`. `multilingual` is `true` when the model accepts a language tag. `primary_language` is the model's default BCP-47 tag (the fallback when no override or global setting applies). `supported_languages` is the non-empty array of BCP-47 tags the model accepts; these feed the per-model language picker and the [`/backends/{source}/models/{model}/language`](./backends/model-language.md) resolution. `supported_devices` is a non-empty array drawn from `["cpu", "cuda", "metal", "none"]`; `"none"` marks a remote/online model with no local compute. `estimated_vram_bytes` is a conservative GPU memory estimate (weights + KV cache + overhead); `0` when unknown or not GPU-resident. See [`GET /gpu_info`](./gpu_info.md) for the detected GPU memory it's weighed against. |
| `…[].secrets`     | array            | Declared secrets: `{ name, label, description, required }`. `label` falls back to `name` when absent. Secret **values** are never returned. |
| `…[].options`     | array            | Declared options: `{ name, label, description, type, default, required, value }`. `label` falls back to `name` when absent; `value` is the effective value (config override if set, else `default`). |

**Errors:**

| HTTP | `message`         | Meaning                                  |
|------|-------------------|------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |

## Per-backend secrets and options

Setting a secret or option is done per item under the backend's sub-resources,
not on `/backends` itself:

- **Secrets** — [`/backends/{source}/secrets`](./backends/secrets.md):
  `GET …/secrets/list` and `GET`/`POST`/`DELETE …/secrets/{name}`. Requires the
  `secrets` scope; values are write-only.
- **Options** — [`/backends/{source}/options`](./backends/options.md):
  `GET …/options/list` and `GET`/`POST`/`DELETE …/options/{name}`. Requires the
  `settings` scope.

For both, `POST` sets a value and `DELETE` resets it to its default — the
manifest default for an option, the unset state for a secret. `GET` on a secret
reports only whether it is configured; `GET` on an option returns its value.

## DELETE /backends/{source}

Uninstalls a backend. Works for any installed backend — registry-installed,
sideloaded, or imported-from-dir. Removes the backend's directory under
`<XDG_DATA_HOME>/super-stt/backends/<id>/` and refreshes the in-memory
discovery list. Idempotent.

### Request

```
DELETE /backends/github.com%2Fjorge-menjivar%2Fsuper-stt
```

The `source` is URL-percent-encoded.

### Response

```json
{ "uninstalled": true, "was_active": false }
```

`was_active` is `true` if this was the active backend. Uninstalling the
active backend first unloads its loaded model (freeing device memory) and
clears the active-backend and preferred-model config, so the daemon goes
fully idle and `GET /status` stays consistent with `GET /active_backend`.

### Failure modes

Errors use the registry error envelope `{ "error": <code> }` (a stable,
machine-readable `code`), matching `POST /registry/install`:

| Status | `error` | Cause |
|---|---|---|
| `404` | `not_found` | No backend with that source is installed. |
| `409` | `backend_busy` | A recording or real-time session is active; the backend set cannot be mutated until it finishes. |
| `500` | `remove_failed` | The backend directory could not be removed (includes a `message`). |
