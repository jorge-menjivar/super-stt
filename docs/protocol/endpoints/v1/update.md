# `/update`

Read the daemon's self-update status: its own version, the latest
published release, and whether an update is available. This is a
read-only snapshot of the last completed check — it does not trigger
a new one. To force an immediate check, use
[`POST /update/check`](./update/check.md).

Candidate selection: the daemon considers the highest semver among
published releases; prereleases are included iff
`beta_optin_effective`; draft releases and tags that don't parse as
semver are always ignored.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `GET /update`

**Request:**

```http
GET /update HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "current_version": "0.2.2-beta.2",
  "latest_version": "v0.2.3-beta.1",
  "update_available": true,
  "checked_at": "2026-08-20T17:00:00Z",
  "last_check_error": null,
  "beta_optin_effective": true,
  "installer_asset": {
    "name": "super-stt-install-x86_64-unknown-linux-gnu",
    "url": "https://github.com/jorge-menjivar/super-stt/releases/download/v0.2.3-beta.1/super-stt-install-x86_64-unknown-linux-gnu",
    "size": 8388608,
    "sha256": "a3f2c8b1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1"
  }
}
```

| Field                   | Type    | Notes                                                                                                                                                                              |
|--------------------------|---------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `current_version`        | string  | The daemon's own workspace version (no `v` prefix).                                                                                                                               |
| `latest_version`         | string? | The candidate release's tag, verbatim (with `v` prefix); `null` before the first completed check.                                                                                 |
| `update_available`       | bool    | Strict semver "candidate > current"; prerelease-aware; never `true` for downgrades or unparsable tags.                                                                            |
| `checked_at`              | string? | RFC 3339 UTC of the last completed check attempt; `null` before the first.                                                                                                         |
| `last_check_error`       | string? | Human-readable failure of the last attempt, `null` on success. A failed check keeps the previous successful result's `latest_version`/`installer_asset` only while `beta_optin_effective` is unchanged from that success; if the effective opt-in changed since (e.g. the `update_beta_optin` setting flipped), the stale candidate is cleared instead of reported alongside the new opt-in. |
| `beta_optin_effective`   | bool    | Resolved from the [`update_beta_optin`](./update_beta_optin.md) setting (`auto` → `true` iff `current_version` is a prerelease).                                                  |
| `installer_asset`        | object? | The `super-stt-install-<target-triple>` asset of the candidate release for this host's architecture; `null` when there is no update, the release lacks the asset, the arch is unsupported, or the release's `SHA256SUMS` asset is unavailable or doesn't list the binary. Clients download this URL to apply the update. |

`installer_asset` fields:

| Field    | Type   | Notes                                                                                     |
|----------|--------|--------------------------------------------------------------------------------------------|
| `name`   | string | Asset file name.                                                                            |
| `url`    | string | Download URL for the asset.                                                                 |
| `size`   | u64    | Asset size in bytes.                                                                         |
| `sha256` | string | Hex SHA-256 of the binary at `url`, from the release's `SHA256SUMS` asset. Always present when `installer_asset` is non-null. Clients MUST verify the downloaded bytes against this before executing the binary. |

**Errors:**

| HTTP | `message`         | Meaning                                  |
|------|-------------------|--------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |
