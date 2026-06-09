# Backend registry

**Date:** 2026-05-29
**Status:** Design — pending implementation

A user-discoverable registry of installable Super STT backends. Users browse and install backends from inside the app instead of finding them on the web. Open submission — community backends ship through the same path as official ones. The registry is a file in this repo; submission is a PR; release tracking is automatic.

## Goal

Replace today's compile-time-bundled `super-stt-app/src/daemon/catalog.rs` (three hardcoded official manifests) and the stub `Message::InstallBackend` with:

- A registry file in this repo (`registry/registry.toml`) listing every known backend.
- A GitHub Actions workflow that builds a static `index.json` and publishes it via GitHub Pages.
- A daemon-side registry client that fetches the index, evaluates per-host compatibility, and performs install / update / uninstall.
- App-side `/registry` endpoints that the Download tab consumes — the app becomes thin, all heavy lifting moves into the daemon.

The maintainer experience is: open a PR once to add a backend, then publish releases on your own repo. No further PRs to bump versions.

## Non-goals (v1)

- **Auto-update.** Update is user-initiated via a button on the Installed card.
- **Pinning to a specific older version.** Latest-per-registry-policy only. The Import-from-dir path is the v1 escape hatch for installing a specific historical build.
- **Sigstore attestations.** The `attestation_url` field is reserved in the index schema but unused; documented as the v2 hardening path.
- **Per-backend sandbox profiles** (subprocess). Stays on the daemon's existing `systemd-run` profile.
- **Cryptographic signing of `index.json`.** TLS-only. Trust = Pages host + indexer-recorded SHA-256s.
- **A web UI for browsing the registry.** The Download tab is the only consumer.
- **Decentralized mirrors / multi-registry support.** One registry, one URL, hardcoded in the daemon (env-overridable for dev/staging only).

## Architecture

```
┌──────────────────────────┐    cron + on-push     ┌──────────────────────┐
│ registry/registry.toml   │ ───────────────────▶ │ GitHub Action        │
│ (one-time PRs add        │                       │ (build_index)        │
│  { id, repo, … })        │                       └──────────┬───────────┘
└──────────────────────────┘                                  │ publish
                                                              ▼
                                                  ┌──────────────────────┐
                                                  │ GitHub Pages         │
                                                  │ index.json (cached)  │
                                                  └──────────┬───────────┘
                                                             │ HTTPS + ETag
                                                             ▼
                                                  ┌──────────────────────┐
                                                  │ super-stt-daemon     │
                                                  │ registry client +    │
                                                  │ compatibility filter │
                                                  │ + install pipeline   │
                                                  └──────────┬───────────┘
                                                             │ GET/POST/DELETE
                                                             ▼
                                                  ┌──────────────────────┐
                                                  │ super-stt-app        │
                                                  │ Download tab         │
                                                  └──────────────────────┘
```

The asset bytes flow `<maintainer's release host> → daemon → <XDG_DATA_HOME>/super-stt/backends/<id>/`. The app never touches the bytes.

## Registry repo layout

Added to the root of `super-stt/super-stt`:

```
registry/
  registry.toml          # the one source of truth — community PRs touch only this
  README.md              # submission instructions, what reviewers check
  .github/workflows/
    build-index.yml      # cron + push trigger
  scripts/
    build_index/         # small Rust crate (the indexer)
```

### `registry.toml`

Entries are alphabetical (CI-enforced). One source of truth — no version pinning here.

```toml
[openai]
repo       = "github.com/jorge-menjivar/super-stt"
subdir     = "backends/openai"     # optional; default: repo root
tag_prefix = "openai-"             # optional; default: matches "v…" or "…"

[mistral]
repo       = "github.com/jorge-menjivar/super-stt"
subdir     = "backends/mistral"
tag_prefix = "mistral-"

[voxtral]
repo       = "github.com/jorge-menjivar/super-stt"
subdir     = "backends/voxtral"
tag_prefix = "voxtral-"

# Yank lever — caps the version the indexer is allowed to publish.
# [some-backend]
# repo        = "github.com/owner/some-backend"
# max_version = "0.3.0"

# Removal — kept for audit, hidden from the published index.
# [abandoned-backend]
# repo           = "github.com/owner/abandoned-backend"
# removed        = true
# removed_reason = "abandoned by maintainer; see issue #1234"
```

**Fields:**

| Field | Required | Notes |
|---|---|---|
| `repo` | yes | GitHub repo URL (no scheme, no trailing slash). |
| `subdir` | no | Path inside the repo where `backend.toml` lives. Validated against `..`, absolute paths, symlinks. |
| `tag_prefix` | no | When set, indexer lists `/releases`, filters tags starting with the prefix, parses the remainder as semver, picks the max. Without it, `GET /releases/latest` is used. |
| `max_version` | no | Hard cap. Anything above is treated as if removed. |
| `removed` | no | `true` hides the entry from the published index. Preserved in `registry.toml` for audit and squatter prevention. |
| `removed_reason` | no | Free text. Surfaced in the indexer's diff log. |

Two registry entries pointing at the same `repo` must both set `tag_prefix` (different values). Indexer hard-rejects on prefix collision or missing prefix in a multi-entry-per-repo situation.

### `backends/` (in-tree)

Unchanged. Stays as a build/CI/dev convenience. The three official backends ship by registering monorepo entries against this same repo with `subdir` + `tag_prefix`. Extracting them to their own repos later is a registry-side two-line change (drop `subdir`, drop `tag_prefix`) with no observable client effect — identity is `(id, repo)`, not layout.

## Per-backend `backend.toml`

Maintainer-published; lives at `<subdir>/backend.toml` in the backend's repo at the release tag. Existing fields stay; assets become the explicit per-variant matrix the indexer reads.

```toml
# Subprocess example
[backend]
source     = "github.com/jorge-menjivar/voxtral"
name       = "Voxtral"
version    = "0.1.0"            # MUST equal the release tag (minus tag_prefix)
kind       = "subprocess"
entrypoint = "voxtral"
contract   = "v1"

[assets]
# kind = "wasm" only:
# wasm = "openai.wasm"

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

# …additional sm/cuda/arch variants…
```

**Allowed values** (declared once in `docs/protocol/backend/config.md`):

- `target` — any Tier-1/2 Rust target triple. Indexer rejects unknown.
- `accel` — `"cpu" | "cuda" | "metal" | "rocm" | "vulkan"`. Adding new values requires a protocol doc PR plus indexer update; not maintainer-extensible.
- `cuda_major`, `cuda_sm` — integer, required iff `accel = "cuda"`. Forbidden otherwise.
- `cudnn` — bool, default `false`. Allowed only when `accel = "cuda"`.

**Packaging:**

- Wasm → raw `.wasm` file. Loaded directly by Wasmtime.
- Subprocess → `.tar.gz` containing `bin/<entrypoint>` (preserves executable bit, compresses, supports multi-file). Extracted into the install dir at install time.

## Index generation workflow

`build-index.yml` triggers on:

- **`schedule:`** every 6 hours.
- **`push:`** to `main` touching `registry/registry.toml` (immediate rebuild on PR merge).
- **`workflow_dispatch:`** manual.

Steps per run:

1. **Parse `registry.toml`.** Schema check, alphabetical sort, no duplicate ids, no prefix collisions.
2. **Fetch the prior `index.json`** from Pages (input to last-known-good carry-forward, step 6).
3. **Per entry, resolve latest tag.** Without `tag_prefix`: `GET /releases/latest`. With: list `/releases`, filter by prefix, semver-sort the remainder, pick max. Respect `max_version` cap. Skip `removed = true`.
4. **Fetch `backend.toml`** from `<repo>/contents/<subdir>/backend.toml?ref=<tag>` (size cap 256 KB).
5. **Per-entry validation gates:**
   - `backend.toml.version` equals the resolved tag (minus `tag_prefix`).
   - `backend.toml.backend.source` equals the registry entry's `repo`.
   - License field present and on the allowlist.
   - Every asset declared in `[assets]` exists on the release.
   - Asset size ≤ 200 MB per variant.
   - Wasm assets carry the `wasm32` magic header.
   - Subprocess tarballs: no path-traversal entries (`..`, absolute), no symlinks escaping the archive, `bin/<entrypoint>` present.
6. **On failure: carry forward.** Copy the entry from the prior `index.json` (if any) with an added `index_stale` field. First-run bootstrap has no prior, so failures drop the entry.
7. **Compute SHA-256** for each asset by streaming download. Record `size` + `sha256` in the new entry.
8. **Diff against prior index.** If changed, commit to the Pages branch.
9. **Failure reporting.** Auto-open a single GitHub issue per failing entry on the registry repo (track open issue number per entry to avoid spam).

Workflow constraints: 5-minute wall-clock cap per run (break early on rate-limit errors and retry next cycle), per-entry limits `backend.toml ≤ 256 KB`, `[models]` array ≤ 50, `[secrets]` ≤ 10, total `index.json` soft-warn 1 MB / hard-fail 5 MB.

## Index schema (`index.json`)

```jsonc
{
  "schema_version": 1,
  "generated_at": "2026-05-29T18:00:00Z",
  "min_client": "0.6.0",                // soft floor; older clients warn
  "backends": [
    {
      "id": "voxtral",
      "source": "github.com/jorge-menjivar/super-stt",
      "version": "0.2.0",
      "tag": "voxtral-0.2.0",
      "name": "Voxtral",
      "description": "…",
      "license": "Apache-2.0",
      "kind": "subprocess",             // or "wasm"
      "contract": "v1",
      "entrypoint": "voxtral",
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
      "assets": {
        "subprocess": [
          { "target": "x86_64-unknown-linux-gnu", "accel": "cpu",
            "url": "…/voxtral-0.2.0/voxtral-x86_64-cpu.tar.gz",
            "size": 12345678, "sha256": "…" },
          { "target": "x86_64-unknown-linux-gnu", "accel": "cuda",
            "cuda_major": 12, "cuda_sm": 86, "cudnn": false,
            "url": "…", "size": 12345678, "sha256": "…" }
        ]
      },
      // Present only when the indexer fell back to last-known-good:
      "index_stale": {
        "latest_attempted": "0.3.0",
        "tag": "voxtral-0.3.0",
        "error": "asset voxtral-x86_64-cpu.tar.gz missing from release",
        "since": "2026-05-29T18:00:00Z"
      },
      // Reserved for v2 sigstore attestations; absent in v1:
      // "attestation_url": "…"
    }
  ]
}
```

The index is self-contained for the Download tab — every field the UI needs (online/CPU/GPU chips, `allowed_hosts`, model list, secrets schema for the Configure subview) is precomputed at index build, not parsed at render time.

## Daemon design

New module: `super-stt-daemon/src/registry/` with three submodules.

### `registry::client`

Fetches and caches `index.json`. URL is hardcoded to `https://jorge-menjivar.github.io/super-stt/index.json` (GitHub Pages serves the registry repo directly — HTTPS, no DNS work, no cert renewal). Env override `SUPER_STT_REGISTRY_URL` for dev/staging, and as the migration path if we ever move off Pages.

- Cache file: `<XDG_CACHE_HOME>/super-stt/registry-index.json`. ETag stored as an extended attribute or sidecar.
- TTL: 6 h (matches index rebuild cadence). On Download-tab open, if cache is stale, fetch in background and emit a refresh event when done.
- On fetch failure: return cache if present; otherwise return an empty list with a typed error. **No compile-time bundled fallback** — single source of truth.

### `registry::compat`

Pure function `select_asset(host: &HostDetect, entry: &RegistryBackend, user_prefs: &Prefs) -> Result<SelectedAsset, IncompatReason>`. Selection algorithm:

1. Filter by exact `target` match.
2. If user prefers GPU **and** a CUDA GPU is detected:
   - Filter `accel = "cuda"`.
   - **Strict** `cuda_sm == detected_sm` (no auto-fallback to lower sm; PTX/JIT availability is per-backend).
   - Among matches, prefer highest `cuda_major ≤ installed CUDA runtime major`.
   - Prefer `cudnn = true` iff cuDNN detected on host.
3. Otherwise (or if step 2 finds nothing): `accel = "cpu"` matching target.
4. None found → `IncompatReason::NoCompatibleAsset { available_variants }`.

`HostDetect` reuses NVML for NVIDIA detection and AMD's sysfs paths for ROCm, the same approach used by `cosmic-utils/minimon-applet` (referenced as prior art).

### `registry::install`

State machine `Idle → Downloading → Verifying → Extracting → Installing → Rescanning → Done | Failed`. Emits progress events via the existing `/events` stream.

Pipeline:

1. Resolve entry from cache (refresh if needed).
2. Run `select_asset`; bail with reason if incompatible.
3. Stream-download the chosen URL to `<XDG_CACHE_HOME>/super-stt/downloads/<id>-<version>-<variant>.{wasm,tar.gz}.partial`.
4. Compute SHA-256 during stream; compare against index. Mismatch ⇒ delete partial, fail loudly. **This is the security-critical check.**
5. Stage into `<XDG_DATA_HOME>/super-stt/backends/.staging/<id>-<version>/`. For `.tar.gz`, extract with path-escape rejection; for `.wasm`, copy.
6. Write the **index-recorded** `backend.toml` (not the one inside the tarball, if any) so the daemon discovers exactly what the registry validated.
7. If upgrading: remove the existing `<XDG_DATA_HOME>/super-stt/backends/<id>/` first.
8. Atomic rename `.staging/<id>-<version>/` → `<id>/`.
9. Call `backends::discover` to refresh the in-memory catalog.
10. Emit `registry.install.completed`.

Failure at any step cleans the staging dir and emits `registry.install.failed` with a typed error.

## Protocol additions

Documented contract-first in `docs/protocol/endpoints/v1/registry/` (the protocol doc lands before the implementation). All wire fields snake_case; invalid values fall back to defaults, no legacy aliases.

| Method | Path | Body / query | Purpose |
|---|---|---|---|
| `GET` | `/registry/backends` | query: `include_incompatible`, `kind=wasm\|subprocess`, `online=true\|false`, `q=<text>` | Filtered registry list. Default: compatible-only. Each entry carries a `compatibility` field; incompatible entries (when included) carry `reason`. |
| `POST` | `/registry/backends/refresh` | — | Force re-fetch of `index.json`, bypassing TTL. Returns the new generation timestamp. |
| `POST` | `/registry/backends/install` | `{ "source": "…" }` or `{ "repo_url": "…" }` (Custom-repo path) | Start install. Returns 202 + `install_id`. Progress on `/events`. |
| `POST` | `/registry/backends/update` | `{ "source": "…" }` | Re-run install if newer version exists; no-op if current. |
| `DELETE` | `/backends/{source}` | — | Uninstall. Lives under `/backends` (not `/registry`) because it works for any installed backend, including sideloaded ones. |

Event types added to `/events`:

- `registry.refresh.completed`, `registry.refresh.failed`
- `registry.install.progress` (`{ install_id, source, phase, bytes_done, bytes_total? }`)
- `registry.install.completed`, `registry.install.failed`

## App-side changes

- `super-stt-app/src/daemon/catalog.rs` is deleted. Replaced by `super-stt-app/src/daemon/registry_client.rs`, a thin client over `GET /registry/backends`.
- `Message::InstallBackend(source)` fires `POST /registry/backends/install`; subscribes to install events; updates the Download-tab card's state.
- New messages: `InstallProgress`, `InstallCompleted`, `InstallFailed`, `RefreshRegistry`.
- Download-tab UI renders whatever the daemon returns. Client-side extra filters (transport, online, search box) become URL params on the next request.
- Custom-repo button uses the same install endpoint with `{ repo_url }`; the daemon surfaces an "unverified source — HTTPS only" warning in the response.
- First-run UX (empty cache, no network): empty Download tab with a "Couldn't reach the registry" panel and Retry button. Custom-repo and Import-from-dir still work.

## Trust + safety

### PR-time gates (`registry.toml`)

- New `id`s don't squat reserved namespaces (`openai`, `anthropic`, etc. — list in `registry/README.md`).
- PR submitter demonstrably controls `repo` (CODEOWNERS or a one-time challenge file at repo HEAD; same trust step Homebrew taps use).
- `subdir` is path-normal (no `..`, no absolute, no symlink at the tag).
- License on the allowlist (Apache-2.0, MIT, BSD-*, GPL-3.0-*, MPL-2.0; others case-by-case).
- `allowed_hosts` (read from the candidate `backend.toml`) — wildcards flagged for reviewer judgment.

### Index-time gates

Listed in the workflow steps above. The critical ones for safety: `backend.source ≡ registry.repo` (no pointing at someone else's release), wasm magic header, tarball path-escape rejection, size caps, license recheck.

### Runtime gates (daemon)

- SHA-256 verification against the index — only defense against in-place asset replacement on GitHub.
- Wasm: Wasmtime + `wasi:http` constrained to index-recorded `allowed_hosts`.
- Subprocess: existing `systemd-run` sandbox.
- `max_version` cap and `removed = true` honored by `GET /registry/backends`.

### Daemon's new outbound surface

Today the daemon's only outbound traffic is wasm-mediated `wasi:http` calls inside backends. After this work, the daemon process itself opens HTTPS to:

- `<pages-host>/index.json`
- `api.github.com` (Custom-repo flow only)
- `objects.githubusercontent.com` + `github.com` (release asset downloads)

No keyring secrets cross those boundaries — secrets are read at model **load** time, not install time. Documented in `docs/SECURITY.md` as part of this work.

### Threats not addressed in v1

- **Compromised maintainer GitHub account.** Auto-tracked latest → malicious release served until manual `max_version` yank. Window is "index rebuild cadence + reaction time." Mitigation later: optional sigstore attestations.
- **Compromised registry maintainer.** They can land any PR. Standard org-trust assumption.
- **Subprocess attack surface > wasm.** `systemd-run` is coarser than `wasi:http`. Mitigation later: tighter per-backend sandbox profiles in `backend.toml`.

## Testing strategy

Every seam is testable without a running daemon for layers 1–4.

1. **Indexer** — pure function `(prior_index, registry.toml, mock_github_responses) → new_index`. Table-driven:
   - tag↔manifest mismatch carries forward last-good with `index_stale`
   - missing asset / oversize manifest / bad license refuses entry
   - asset SHA-256 differs from prior run → entry re-published with new hash
   - `max_version` cap; `removed = true` drops
   - monorepo `tag_prefix` resolution; prefix collision rejected
   - first-run bootstrap (no prior index) drops failures cleanly
2. **Compatibility** — pure function `select_asset(host, entry, prefs) → SelectedAsset | Reason`. Table-driven across `target × accel × cuda_major × cuda_sm × cudnn`. Cover: sm-not-in-set, cudnn-not-detected, cpu-only-host-with-only-cuda-assets, GPU-preferred-but-no-CUDA-asset-falls-back-to-CPU.
3. **Registry client (daemon)** — `mockito` HTTP server. Cold start → fetch → cache hit → TTL expiry → ETag 304 → network failure falls back to cache → no cache *and* no network = empty list + typed error.
4. **Install pipeline** — fake index + fake asset host. Security-critical: *hash mismatch refuses install and leaves no bytes on disk*. Also: partial-download resume, staging dir cleanup on failure, atomic rename on success, uninstall-then-reinstall = upgrade, executable bit survives tar.gz extraction, path-escape entries rejected during extract.
5. **Daemon endpoints** — `--lib` HTTP integration tests for `GET /registry/backends` (with each query param), install/update/uninstall flows, event-stream payloads. The keyring lock caveat that affects daemon HTTP integration tests still applies — automated runs use `cargo test --lib`.
6. **PR-time CI on the registry repo** — lint suite implemented as a Rust binary so its rules are unit-tested: `registry.toml` schema, sorted, no duplicate ids, no path traversal in `subdir`, license allowlist, allowed_hosts wildcard warnings, prefix-collision detection.

## Open questions for implementation

- **Reserved-id list.** `registry/README.md` needs an initial list before opening to community PRs.
- **`GITHUB_TOKEN` scope.** Indexer needs `contents:read` across arbitrary repos for `backend.toml` lookups. Verify the default workflow token suffices for public-repo reads (it does for `/contents`, `/releases`); document explicitly.
