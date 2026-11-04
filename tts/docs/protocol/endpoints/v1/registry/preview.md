# `POST /registry/backend/preview`

Resolves an install source and describes the backend it would install, without
installing it. Same body as
[`/registry/backend/install`](install.md), same resolution, and the answer is
the shape [`/registry/backend/list`](backends.md) already returns.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## Request

The three body shapes `/registry/backend/install` accepts — provide exactly one.

```json
{ "repo_url": "github.com/your-name/your-backend" }
```

```json
{ "local_path": "/home/alice/dev/my-backend" }
```

```json
{ "source": "github.com/jorge-menjivar/super-tts-kokoro" }
```

`forge` is accepted alongside `repo_url` and means what it means on install: the
host in the URL picks the adapter when it is absent.

## Response

```json
{
  "backend": {
    "id": "app.super-tts.kokoro",
    "source": "github.com/jorge-menjivar/super-tts-kokoro",
    "name": "Kokoro",
    "version": "0.1.0",
    "description": "Kokoro-82M local text-to-speech, 24 kHz, 54 preset voices.",
    "kind": "subprocess",
    "license": "Apache-2.0",
    "allowed_hosts": [],
    "online": false,
    "models": [{ "name": "kokoro-82m", "supported_devices": ["cpu"] }],
    "secrets": [],
    "compatibility": { "compatible": true, "selected_asset": { "target": "x86_64-unknown-linux-gnu", "accel": ["cpu"] } }
  },
  "warning": "unverified_source"
}
```

Status: `200 OK`.

`backend` is a `RegistryBackend`, field-identical to a
`/registry/backend/list` entry, so a client renders a preview with whatever it
renders a catalog entry with. `compatibility` is decided against this host
exactly as the listing decides it.

`warning` carries `unverified_source` for the custom-repo and local-import
routes, matching the install response. It is absent for a registry `source`.

**An incompatible host is not an error here.** `/registry/backend/install`
answers `422 incompatible` because there is nothing it can do; a preview
answers `200` with `compatibility.compatible = false` and a `reason`, because
"this machine cannot run it" is the most useful thing a preview can say and
saying it needs the entry.

**Nothing is written.** No inflight marker, no background pipeline, nothing on
disk. Two calls in a row are the same as one, and a preview never conflicts
with an install already running for the same source.

## Why it exists

`/registry/backend/install` answers `202` as soon as it has chosen an asset,
which is *after* the point of no return. A person pasting a repository URL has
no other way to see what is in it before committing: the name, the version,
what it runs as, what it can reach, whether this machine can run it. The
Super TTS app calls this to fill the Install-manually drawer's preview, and
shows failures in the same panel.

## Failure modes

| Status | Cause |
|---|---|
| `400` | Body has zero or more than one of `source` / `repo_url` / `local_path`. Body: `{"error":"bad_request"}`. For Custom-repo, `repo_url` not a `<host>/<owner>/<repo>` reference: `{"error":"bad_repo_url"}`; no forge adapter serves its host and no `forge` was sent: `{"error":"unsupported_forge"}`. For Import-from-dir, `local_path` not an absolute path: `{"error":"bad_local_path"}`. |
| `404` | `source` not in the cached or refreshed index, or the repo has no published release, or `<local_path>` / its `backend.toml` / its entrypoint does not exist: `{"error":"not_found"}`. |
| `422` | `backend.toml` invalid: `{"error":"manifest_invalid"}` or `{"error":"manifest_too_large"}`; a declared asset is missing from the release: `{"error":"asset_missing"}`; the manifest's `source` is not the repo it was fetched from (identity spoofing): `{"error":"source_mismatch"}`. |
| `502` | Custom-repo: forge API unreachable. Body: `{"error":"forge_unavailable"}`. |
| `503` | Registry index unreachable and no cache. Body: `{"error":"registry_unavailable"}`. |
