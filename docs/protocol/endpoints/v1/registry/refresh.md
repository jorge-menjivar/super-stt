# POST /registry/backends/refresh

Forces an immediate re-fetch of the registry index, bypassing the TTL.
Idempotent — concurrent requests coalesce into a single in-flight fetch.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## Request

```
POST /registry/backends/refresh
```

No body.

## Response

```json
{
  "schema_version": 1,
  "generated_at": "2026-05-29T18:00:00Z",
  "backend_count": 7
}
```

## Failure modes

| Status | Cause |
|---|---|
| `503` | Could not reach the registry. Body: `{"error":"registry_unavailable"}`. |
