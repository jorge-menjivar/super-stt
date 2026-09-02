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
      "backend_id": "app.super-stt.voxtral",
      "source": "github.com/jorge-menjivar/super-stt",
      "version": "0.2.0",
      "name": "Voxtral",
      "description": "…",
      "license": "Apache-2.0",
      "kind": "subprocess",
      "contract": "v1",
      "min_client": "0.1.0",
      "allowed_hosts": [],
      "online": false,
      "supports_gpu": true,
      "supports_cpu": true,
      "models": [
        { "name": "voxtral-mini", "provider": "",
          "supported_devices": ["cpu", "gpu"] }
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
      "installed_version": "0.1.0",
      "update_available": true
    }
  ]
}
```

`id` is the registry key from `registry.toml`. `backend_id` is the backend's
reverse-DNS identifier, declared by `[backend].id` in its release manifest —
`null` for an entry that predates the field. The two are maintained
independently: neither is derived from the other.

The install directory is named by `backend_id`, falling back to `id` when
`backend_id` is `null` — see
[`POST /registry/install`](./install.md#request) for the full rule. A client
computing the path must apply that fallback rather than assume either field.

`models[].provider` is always an empty string. It is emitted so clients that
require the key can still parse the response, and carries no information —
identify a model by `(name, source)` instead. It will be removed.

`models[].supported_devices` names the runtimes a model can use: `"cpu"`,
`"gpu"`, or the `"none"` sentinel for a model that runs remotely. It is a
property of the *model*, not of this host — see
[`GET /backends`](../backends.md) for narrowing it to the devices the
installed build actually provides.

Per-entry fields beyond what `index.json` carries:

- `compatibility.compatible` — `true` if a matching asset exists for this host.
- `compatibility.selected_asset` — the asset the daemon would install. Only
  the selection axes (target/accel/cuda_*/cudnn) are reported; URL + hash are
  internal. `accel` is a string for a build carrying one runtime, and an array
  for one carrying several — the same two spellings
  [`backend.toml`](../../../backend/config.md#assets) accepts.
- `compatibility.reason` — present only when `compatible = false`. Human-readable.
  A backend whose [`contract`](../../../backend/config.md#contract-generations)
  generation this daemon does not know is reported here too — before any
  asset question — as `needs Super STT <min_client> or newer (backend contract
  <contract>); this is <daemon version>`. `POST .../install` and
  `POST .../update` return the same sentence as their `422 incompatible`
  `message`, so the fix is stated wherever the block is met.
- `compatibility.needs_client_update` — `true` when the block is the contract
  generation rather than this host's hardware. Such an entry is listed even
  without `include_incompatible`: the toggle is for hardware a machine will
  never satisfy, and the remedy here is an update the user can act on.
- `installed_version` — present if the backend is already installed on this
  host, regardless of its registry status. Read from the installed
  `backend.toml` on every request, so it reflects what is on disk now rather
  than what the daemon saw at startup. It is the same read that fills
  [`version` on `GET /backends`](../backends.md), so the two never disagree.
- `update_available` — whether `version` is newer than `installed_version`,
  compared as semver. The daemon decides this rather than leaving each client
  to re-derive it: the daemon is the side that reads the installed manifest and
  owns the index, so it is the only one that can answer without duplicating
  both. The daemon matches an installed backend to this entry by `source`, so
  a backend installed from a custom repository or a local directory is matched
  the same way one installed from the registry is. `false` when nothing is
  installed, when the installed version is at or ahead of the index's, or when
  either version does not parse — so a stale or older index never advertises a
  downgrade. Clients that want to *show* the versions still have both fields.

## Failure modes

| Status | Cause |
|---|---|
| `503` | Registry index unreachable and no cache. Body: `{"error":"registry_unavailable"}`. |
| `200` with empty `backends` | Registry reachable, but no entries match the filters. |
