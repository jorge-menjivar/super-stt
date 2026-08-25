# POST /registry/backends/install

Installs a backend from the registry, from an arbitrary git-forge repository
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
{ "repo_url": "github.com/your-name/your-backend", "forge": "github" }
```

The daemon queries the declared forge's API for the repo's latest release,
downloads its `backend.toml` release asset, runs the same selection algorithm
over the declared binary assets, and installs the manifest verbatim. **The
manifest and assets are not hash-verified against any registry** — TLS to the
forge is the only integrity guarantee (the daemon still parses, validates, and
identity-checks the manifest). The synchronous response carries
`warning: "unverified_source"`; clients should surface this in the UI.

**Import-from-dir install:**
```json
{ "local_path": "/home/alice/dev/my-backend" }
```

The daemon reads `<local_path>/backend.toml`, validates the manifest, and
copies the backend into the backends directory as if a registry install had
completed. No network access. No checksum verification — the operator chose the
bytes. Symlinks are rejected (so an import cannot copy a link target's bytes
into the install dir). The synchronous response carries
`warning: "unverified_source"`; the `selected_asset` reflects the local copy
with `accel = "local"` and an empty `target`.

What gets copied depends on `[backend].kind`, and mirrors what a registry
install of that kind produces:

| Kind         | Copied                                                                 |
|--------------|------------------------------------------------------------------------|
| `wasm`       | `backend.toml` and the file `[backend].entrypoint` names — nothing else. |
| `subprocess` | The directory tree, minus VCS metadata (`.git`).                        |

A `wasm` install is exactly those two files, so the import takes them and
ignores everything beside them: pointing `local_path` at a source checkout
copies neither the build tree nor the repository history.

A `subprocess` executable may need siblings no manifest field declares — a
bundled interpreter, shared libraries, resource files — and the registry
equivalent is an opaque tarball, so the whole tree is taken. Stage a
subprocess backend as the directory you would have tarred, not as a source
checkout: everything beside the executable is copied verbatim.

Model files are not part of either copy. The daemon downloads each
[`[[models.files]]`](../../../backend/config.md#modelsfiles) entry into its
`destination` at load time.

The directory must already contain the file `[backend].entrypoint` names — the
`.wasm` component or the executable. A registry release ships it built and
named; an import is staged by the operator, so nothing else establishes that it
is there. Without this check the install would succeed and the backend would
fail at model load with a read error naming a path, long after the operator
could connect it to what they staged. A source checkout is the usual cause: a
build tree names its artifact after the crate, not after the entrypoint.

**Install directory.** A backend is installed into a directory named by its
`[backend].id`, whichever route installed it — the registry, a custom
repository, or a local directory. A backend whose manifest declares no `id`
is installed under the registry key instead, so installs that predate the
identifier keep their directory.

That directory must be free for this backend to take. An install whose target
directory already holds a backend declaring a different `[backend].source`
fails with `install_dir_conflict`; both directories are left exactly as they
were, and no model files are moved. Completing such an install would replace
the backend already there and delete the model files downloaded under it.
`source` is the identity this is judged on, so re-installing or updating the
same backend is unaffected, as is replacing an install whose `backend.toml`
no longer parses.

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
- `install_dir_conflict` — the install directory already holds a backend
  declaring a different `[backend].source`; nothing on disk was changed

## Failure modes (synchronous)

| Status | Cause |
|---|---|
| `400` | Body has zero or more than one of `source` / `repo_url` / `local_path`. Body: `{"error":"bad_request"}`. For Custom-repo, `repo_url` not a `<host>/<owner>/<repo>` reference: `{"error":"bad_repo_url"}`. Custom-repo `forge` missing: `{"error":"bad_request"}` (an unrecognized `forge` value is rejected earlier as a malformed body). For Import-from-dir, `local_path` not an absolute path: `{"error":"bad_local_path"}`. |
| `404` | `source` not in the cached or refreshed index, or Custom-repo repo/release/`backend.toml` not found at the forge: `{"error":"not_found"}`. For Import-from-dir, `<local_path>`, its `backend.toml`, or the file `[backend].entrypoint` names does not exist: `{"error":"not_found"}`. |
| `409` | An install for this `source` is already in flight. Body: `{"error":"install_in_progress"}`. |
| `422` | No compatible asset on this host: `{"error":"incompatible"}`. For Custom-repo, `backend.toml` invalid: `{"error":"manifest_invalid"}` or `{"error":"manifest_too_large"}`; a declared asset is missing from the release: `{"error":"asset_missing"}`; the manifest's `source` is not the repo it was fetched from or namespaced under it (identity spoofing): `{"error":"source_mismatch"}`. For Import-from-dir, `<local_path>/backend.toml` failed to parse or yields an unsafe install id: `{"error":"manifest_invalid"}`. |
| `502` | Custom-repo: forge API unreachable. Body: `{"error":"forge_unavailable"}`. |
| `503` | Registry index unreachable and no cache. Body: `{"error":"registry_unavailable"}`. |
