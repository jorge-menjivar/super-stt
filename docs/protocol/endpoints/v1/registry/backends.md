# GET /registry/backends

Lists installable backends from the registry. The daemon fetches the registry
index from a hardcoded GitHub Pages URL (see
`docs/superpowers/specs/2026-05-29-backend-registry-design.md`) and applies
per-host compatibility filtering before responding. By default, incompatible
entries are excluded; the client can opt in to seeing them with
`include_incompatible=true`.

The full registry catalog (every entry the index publishes) is **not**
exposed verbatim — the daemon only returns what's installable on the current
host, plus the index's own metadata (generation time, schema version,
optional `index_stale` marker on a per-entry basis).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## Request

```
GET /registry/backends?include_incompatible=false&kind=wasm&online=true&q=openai
```

| Query parameter | Type | Default | Notes |
|---|---|---|---|
| `include_incompatible` | bool | `false` | When `true`, entries with no compatible asset are returned with `compatibility.compatible = false` and a `compatibility.reason`. |
| `kind` | `"wasm" \| "subprocess"` | (none) | Filter by transport. |
| `online` | bool | (none) | Filter by online vs local. |
| `q` | string | (none) | Case-insensitive substring match against `name` and `description`. |

## Response

```json
{
  "schema_version": 1,
  "generated_at": "2026-05-29T18:00:00Z",
  "backends": [
    {
      "id": "voxtral",
      "source": "github.com/jorge-menjivar/super-stt",
      "version": "0.2.0",
      "name": "Voxtral",
      "description": "…",
      "license": "Apache-2.0",
      "kind": "subprocess",
      "contract": "v1",
      "allowed_hosts": [],
      "online": false,
      "supports_gpu": true,
      "supports_cpu": true,
      "models": [
        { "name": "voxtral-mini", "provider": "voxtral",
          "supported_devices": ["cpu", "cuda"] }
      ],
      "secrets": [],
      "options": [],
      "compatibility": {
        "compatible": true,
        "selected_asset": {
          "target": "x86_64-unknown-linux-gnu",
          "accel": "cuda",
          "cuda_major": 12,
          "cuda_sm": 86,
          "cudnn": false
        }
      },
      "installed_version": "0.1.0"
    }
  ]
}
```

Per-entry fields beyond what `index.json` carries:

- `compatibility.compatible` — `true` if a matching asset exists for this host.
- `compatibility.selected_asset` — the asset the daemon would install. Only
  the selection axes (target/accel/cuda_*/cudnn) are reported; URL + hash are
  internal.
- `compatibility.reason` — present only when `compatible = false`. Human-readable.
- `installed_version` — present if the backend is already installed on this
  host, regardless of its registry status.

## Failure modes

| Status | Cause |
|---|---|
| `503` | Registry index unreachable and no cache. Body: `{"error":"registry_unavailable"}`. |
| `200` with empty `backends` | Registry reachable, but no entries match the filters. |
