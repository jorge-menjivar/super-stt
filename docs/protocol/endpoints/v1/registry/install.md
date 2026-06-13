# POST /registry/backends/install

Installs a backend from the registry, from an arbitrary GitHub repository
(Custom-repo path), or from a locally staged directory (Import-from-dir
path). Returns immediately with an `install_id`; the actual install runs in
the background. Progress is delivered on the `/events` stream as
`registry.install.progress`, `.completed`, `.failed`.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## Request

Three body shapes — provide exactly one.

**Registry install:**
```json
{ "source": "github.com/jorge-menjivar/super-stt" }
```

The daemon looks up the entry whose `source` matches and installs its
selected asset. If the entry is not in the cached index, the daemon does a
single inline refresh before failing.

The installed `backend.toml` is the backend's own manifest, published as a
pinned release asset (the `manifest` field on every index entry). The daemon
downloads it, verifies its SHA-256 against the pin, confirms it parses, passes
runtime validation, and declares a `source` and `entrypoint` matching the index
entry — then installs those exact bytes (a backend cannot pin a manifest that
claims another backend's identity). There is no synthesized-manifest fallback: a
registry entry without a `manifest` pin is not installable.

**Custom-repo install:**
```json
{ "repo_url": "github.com/your-name/your-backend" }
```

The daemon queries the GitHub REST API for the repo's latest release, downloads
its `backend.toml` release asset, runs the same selection algorithm over the
declared binary assets, and installs the manifest verbatim. **The manifest and
assets are not hash-verified against any registry** — TLS to GitHub is the only
integrity guarantee (the daemon still parses, validates, and identity-checks the
manifest). The synchronous response carries `warning: "unverified_source"`;
clients should surface this in the UI.

**Import-from-dir install:**
```json
{ "local_path": "/home/alice/dev/my-backend" }
```

The daemon reads `<local_path>/backend.toml`, validates the manifest, and
copies the directory contents into the backends directory as if a registry
install had completed. No network access. No checksum verification — the
operator chose the bytes. Symlinks in the directory are rejected (so an import
cannot copy a link target's bytes into the install dir). The synchronous
response carries `warning: "unverified_source"`; the `selected_asset` reflects
the local copy with `accel = "local"` and an empty `target`.

**Integrity & limits.** Operator base-URL overrides (`GITHUB_API_BASE`,
`SUPER_STT_REGISTRY_URL`) must be `https://` (loopback `http://` is allowed for
testing); insecure values are ignored and the secure default is used. Downloads
are capped at the index-declared asset size plus a small margin (or an absolute
ceiling when no size is declared), and tarball extraction enforces per-file and
total-output budgets — an asset that streams past its declared size, or an
archive that decompresses beyond the budget, fails the install.

## Response

```json
{
  "install_id": "ins_01HE5…",
  "source": "github.com/jorge-menjivar/super-stt",
  "version": "0.2.0",
  "selected_asset": {
    "target": "x86_64-unknown-linux-gnu",
    "accel": "cuda",
    "cuda_major": 12,
    "cuda_sm": 86,
    "cudnn": false
  },
  "warning": null
}
```

Status: `202 Accepted`. Progress and outcome follow on the event stream.

## Events

Streamed on `GET /events`:

```jsonc
// registry.install.progress
{
  "type": "registry.install.progress",
  "install_id": "ins_01HE5…",
  "source": "github.com/…",
  "phase": "downloading" | "verifying" | "extracting" | "installing" | "rescanning",
  "bytes_done": 1234567,        // present only in `downloading`
  "bytes_total": 12345678        // may be null if Content-Length missing
}

// registry.install.completed
{
  "type": "registry.install.completed",
  "install_id": "ins_01HE5…",
  "source": "github.com/…",
  "version": "0.2.0"
}

// registry.install.failed
{
  "type": "registry.install.failed",
  "install_id": "ins_01HE5…",
  "source": "github.com/…",
  "phase": "verifying",
  "error": "asset_hash_mismatch"
}
```

Typed `error` values:

- `incompatible` — no asset matches this host
- `download_failed` — HTTP error during asset download
- `asset_hash_mismatch` — SHA-256 from the index didn't match
- `tarball_unsafe` — path-traversal or symlink-escape entry
- `install_io_error` — extraction or rename failed; details in `message`
- `manifest_invalid` — the `backend.toml` asset was absent or failed
  verification: no `manifest` pin, unparseable, failed runtime validation, over
  the size cap, or a `source`/`entrypoint` inconsistent with the index entry

## Failure modes (synchronous)

| Status | Cause |
|---|---|
| `400` | Body has zero or more than one of `source` / `repo_url` / `local_path`. Body: `{"error":"bad_request"}`. For Custom-repo, `repo_url` not a `github.com/<owner>/<repo>` reference: `{"error":"bad_repo_url"}`. For Import-from-dir, `local_path` not an absolute path: `{"error":"bad_local_path"}`. |
| `404` | `source` not in the cached or refreshed index, or Custom-repo repo/release/`backend.toml` not found at GitHub: `{"error":"not_found"}`. For Import-from-dir, `<local_path>` or its `backend.toml` does not exist: `{"error":"not_found"}`. |
| `409` | An install for this `source` is already in flight. Body: `{"error":"install_in_progress"}`. |
| `422` | No compatible asset on this host: `{"error":"incompatible"}`. For Custom-repo, `backend.toml` invalid: `{"error":"manifest_invalid"}` or `{"error":"manifest_too_large"}`; a declared asset is missing from the release: `{"error":"asset_missing"}`; the manifest's `source` is not the repo it was fetched from or namespaced under it (identity spoofing): `{"error":"source_mismatch"}`. For Import-from-dir, `<local_path>/backend.toml` failed to parse or yields an unsafe install id: `{"error":"manifest_invalid"}`. |
| `502` | Custom-repo: GitHub API unreachable. Body: `{"error":"github_unavailable"}`. |
| `503` | Registry index unreachable and no cache. Body: `{"error":"registry_unavailable"}`. |
