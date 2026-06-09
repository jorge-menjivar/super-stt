# POST /registry/backends/update

Re-runs the install pipeline if the registry's version is newer than the
installed version. No-op if already current.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## Request

```json
{ "source": "github.com/jorge-menjivar/super-stt" }
```

## Response

```json
{
  "install_id": "ins_01HE5…",
  "from_version": "0.1.0",
  "to_version": "0.2.0",
  "noop": false
}
```

When `noop = true`, `install_id` is absent and `from_version == to_version`.

Progress events follow the same shape as `/registry/backends/install`.

## Failure modes

| Status | Cause |
|---|---|
| `404` | Source not installed, or not in the registry. |
| `409` | Update already in flight. |
