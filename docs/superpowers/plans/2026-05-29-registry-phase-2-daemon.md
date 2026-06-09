# Registry Phase 2: Daemon Registry Module + Endpoints Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `registry` module to `super-stt-daemon` that fetches the live `index.json`, evaluates per-host compatibility, and runs install / update / uninstall. Expose this over the new `/registry/backends` endpoints. After Phase 2, you can `curl` the daemon to list installable backends and install one — no app changes yet.

**Architecture:** Three submodules — `client` (fetch + cache `index.json` with ETag), `compat` (pure `select_asset(host, entry, prefs)`), `install` (state-machine pipeline: download → hash → extract → install → discover). HTTP routes added under `/registry/backends/*`. Install progress emitted on the existing `/events` stream.

**Tech Stack:**
- Reuses existing daemon stack: `axum`/`hyper` (already in `http_server.rs`), `reqwest`, `ring`, `tokio`, `tar`, `flate2`
- New: `semver` (already in workspace via Phase 1; add to daemon)
- Reuses `nvml-wrapper` or `nvml-rs` style detection — daemon already opens NVML elsewhere; verify before importing

---

## File Structure

**Create:**
- `docs/protocol/endpoints/v1/registry/backends.md` — `GET /registry/backends`
- `docs/protocol/endpoints/v1/registry/refresh.md` — `POST /registry/backends/refresh`
- `docs/protocol/endpoints/v1/registry/install.md` — `POST /registry/backends/install` + events
- `docs/protocol/endpoints/v1/registry/update.md` — `POST /registry/backends/update`
- `docs/protocol/endpoints/v1/backends.md` (modify, see below) — add `DELETE /backends/{source}`
- `super-stt-daemon/src/registry/mod.rs` — module entry
- `super-stt-daemon/src/registry/index_schema.rs` — `index.json` deserialization (same shape as Phase 1's output)
- `super-stt-daemon/src/registry/client.rs` — fetch + cache
- `super-stt-daemon/src/registry/host_detect.rs` — target triple + CUDA sm + cuDNN detection
- `super-stt-daemon/src/registry/compat.rs` — `select_asset`
- `super-stt-daemon/src/registry/install.rs` — pipeline state machine
- `super-stt-shared/src/registry/mod.rs` — wire types (DTOs for `/registry/backends`)
- `super-stt-shared/src/registry/events.rs` — `registry.install.*` event types

**Modify:**
- `super-stt-daemon/Cargo.toml` — add `semver`, `tar`, `flate2`
- `super-stt-daemon/src/daemon/http_server.rs` — register routes
- `super-stt-daemon/src/daemon/handlers.rs` — handler functions
- `super-stt-daemon/src/daemon/events.rs` (or wherever events are typed) — add install events
- `docs/SECURITY.md` — daemon outbound surface note

---

### Task 1: Document `GET /registry/backends`

**Files:**
- Create: `docs/protocol/endpoints/v1/registry/backends.md`

- [ ] **Step 1: Read the existing endpoint docs for house style**

Read `docs/protocol/endpoints/v1/backends.md` and `docs/protocol/endpoints/v1/active_backend.md`. Note: each doc opens with a 1–2 paragraph description, then `## Request` / `## Response` sections with JSON examples, then a "Failure modes" table.

- [ ] **Step 2: Write the new doc**

```markdown
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
```

- [ ] **Step 3: Commit**

```bash
git add docs/protocol/endpoints/v1/registry/backends.md
git commit -m "docs(protocol): GET /registry/backends"
```

---

### Task 2: Document `POST /registry/backends/refresh`

**Files:**
- Create: `docs/protocol/endpoints/v1/registry/refresh.md`

- [ ] **Step 1: Write the doc**

```markdown
# POST /registry/backends/refresh

Forces an immediate re-fetch of the registry index, bypassing the TTL.
Idempotent — concurrent requests coalesce into a single in-flight fetch.

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
| `503` | Could not reach the registry. Body: `{"error":"registry_unavailable", "cached_generated_at": "…"}` (last cached `generated_at`, if any). |
```

- [ ] **Step 2: Commit**

```bash
git add docs/protocol/endpoints/v1/registry/refresh.md
git commit -m "docs(protocol): POST /registry/backends/refresh"
```

---

### Task 3: Document `POST /registry/backends/install` and install events

**Files:**
- Create: `docs/protocol/endpoints/v1/registry/install.md`

- [ ] **Step 1: Write the doc**

```markdown
# POST /registry/backends/install

Installs a backend from the registry, or installs from an arbitrary GitHub
repository (Custom-repo path). Returns immediately with an `install_id`;
the actual install runs in the background. Progress is delivered on the
`/events` stream as `registry.install.progress`, `.completed`, `.failed`.

## Request

Two body shapes:

**Registry install:**
```json
{ "source": "github.com/jorge-menjivar/super-stt" }
```

The daemon looks up the entry whose `source` matches and installs its
selected asset. If the entry is not in the cached index, the daemon does a
single inline refresh before failing.

**Custom-repo install:**
```json
{ "repo_url": "github.com/your-name/your-backend" }
```

The daemon queries the GitHub REST API for the repo's latest release, fetches
its `backend.toml`, runs the same selection algorithm, and downloads the
chosen asset. **The asset is not hash-verified against any registry** — TLS
to GitHub is the only integrity guarantee. The response includes a
`warning: "unverified_source"` field, and the install events likewise carry
`unverified_source: true`.

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
  "phase": "verifying",         // phase the failure occurred in
  "error": "asset_hash_mismatch"  // typed string; see below
}
```

Typed `error` values:

- `incompatible` — no asset matches this host
- `download_failed` — HTTP error during asset download
- `asset_hash_mismatch` — SHA-256 from the index didn't match
- `tarball_unsafe` — path-traversal or symlink-escape entry
- `install_io_error` — extraction or rename failed; details in `message`

## Failure modes (synchronous)

| Status | Cause |
|---|---|
| `404` | `source` not in the cached or refreshed index. |
| `400` | Body missing both `source` and `repo_url`, or both present. |
| `409` | An install for this `source` is already in flight. |
```

- [ ] **Step 2: Commit**

```bash
git add docs/protocol/endpoints/v1/registry/install.md
git commit -m "docs(protocol): POST /registry/backends/install + events"
```

---

### Task 4: Document `POST /registry/backends/update` and modify `/backends/{source}` for `DELETE`

**Files:**
- Create: `docs/protocol/endpoints/v1/registry/update.md`
- Modify: `docs/protocol/endpoints/v1/backends.md`

- [ ] **Step 1: Write `update.md`**

```markdown
# POST /registry/backends/update

Re-runs the install pipeline if the registry's version is newer than the
installed version. No-op if already current.

## Request

```json
{ "source": "github.com/jorge-menjivar/super-stt" }
```

## Response

```json
{
  "install_id": "ins_01HE5…",   // present iff an update is in flight
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
```

- [ ] **Step 2: Add `DELETE` to `backends.md`**

Open `docs/protocol/endpoints/v1/backends.md`. Append a new section:

```markdown
## DELETE /backends/{source}

Uninstalls a backend. Works for any installed backend — registry-installed,
sideloaded, or imported-from-dir. Removes the backend's directory under
`<XDG_DATA_HOME>/super-stt/backends/<id>/` and refreshes the in-memory
discovery list. Idempotent.

### Request

```
DELETE /backends/github.com/jorge-menjivar/super-stt
```

The `source` is URL-percent-encoded.

### Response

```json
{ "uninstalled": true, "was_active": false }
```

`was_active` is `true` if this was the active backend, which is cleared by
the uninstall (daemon goes idle).

### Failure modes

| Status | Cause |
|---|---|
| `404` | No backend with that source is installed. |
```

- [ ] **Step 3: Commit**

```bash
git add docs/protocol/endpoints/v1/registry/update.md docs/protocol/endpoints/v1/backends.md
git commit -m "docs(protocol): POST /registry/backends/update + DELETE /backends/{source}"
```

---

### Task 5: Add registry wire types to `super-stt-shared`

**Files:**
- Create: `super-stt-shared/src/registry/mod.rs`
- Create: `super-stt-shared/src/registry/events.rs`
- Modify: `super-stt-shared/src/lib.rs`

- [ ] **Step 1: Write `super-stt-shared/src/registry/mod.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Wire types for `/registry/backends` and friends. All fields snake_case.

use serde::{Deserialize, Serialize};

pub mod events;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryListResponse {
    pub schema_version: u32,
    pub generated_at: String,
    pub backends: Vec<RegistryBackend>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryBackend {
    pub id: String,
    pub source: String,
    pub version: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub license: String,
    pub kind: String,
    pub contract: String,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    pub online: bool,
    pub supports_gpu: bool,
    pub supports_cpu: bool,
    pub models: Vec<RegistryModel>,
    pub secrets: Vec<RegistrySecret>,
    pub options: Vec<RegistryOption>,
    pub compatibility: Compatibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_stale: Option<IndexStale>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryModel {
    pub name: String,
    pub provider: String,
    pub supported_devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrySecret {
    pub name: String,
    pub label: String,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryOption {
    pub name: String,
    pub label: String,
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compatibility {
    pub compatible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_asset: Option<SelectedAsset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectedAsset {
    pub target: String,
    pub accel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_major: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_sm: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cudnn: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStale {
    pub latest_attempted: String,
    pub tag: String,
    pub error: String,
    pub since: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InstallRequest {
    BySource { source: String },
    ByRepoUrl { repo_url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallAccepted {
    pub install_id: String,
    pub source: String,
    pub version: String,
    pub selected_asset: SelectedAsset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRequest {
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
    pub from_version: String,
    pub to_version: String,
    pub noop: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshResponse {
    pub schema_version: u32,
    pub generated_at: String,
    pub backend_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UninstallResponse {
    pub uninstalled: bool,
    pub was_active: bool,
}
```

- [ ] **Step 2: Write `events.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Event payloads streamed on `/events` for registry install progress.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RegistryEvent {
    #[serde(rename = "registry.install.progress")]
    Progress {
        install_id: String,
        source: String,
        phase: InstallPhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bytes_done: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bytes_total: Option<u64>,
    },
    #[serde(rename = "registry.install.completed")]
    Completed { install_id: String, source: String, version: String },
    #[serde(rename = "registry.install.failed")]
    Failed { install_id: String, source: String, phase: InstallPhase, error: InstallError },
    #[serde(rename = "registry.refresh.completed")]
    RefreshCompleted { generated_at: String, backend_count: usize },
    #[serde(rename = "registry.refresh.failed")]
    RefreshFailed { error: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallPhase {
    Resolving,
    Downloading,
    Verifying,
    Extracting,
    Installing,
    Rescanning,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallError {
    Incompatible,
    DownloadFailed,
    AssetHashMismatch,
    TarballUnsafe,
    InstallIoError,
}
```

- [ ] **Step 3: Register the module in `super-stt-shared/src/lib.rs`**

Add `pub mod registry;` next to the other top-level `pub mod` declarations.

- [ ] **Step 4: Build the workspace**

Run: `cargo check --workspace`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add super-stt-shared/
git commit -m "feat(shared): registry wire types + install event payloads"
```

---

### Task 6: Index deserialization (in daemon, mirrors Phase 1 output)

**Files:**
- Create: `super-stt-daemon/src/registry/mod.rs`
- Create: `super-stt-daemon/src/registry/index_schema.rs`

- [ ] **Step 1: Add `semver` to the daemon**

```bash
cd super-stt-daemon && cargo add semver
```

- [ ] **Step 2: Write `mod.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Daemon-side registry client, compatibility evaluation, and install pipeline.

pub mod client;
pub mod compat;
pub mod host_detect;
pub mod index_schema;
pub mod install;
```

- [ ] **Step 3: Write `index_schema.rs` — same shape as Phase 1's `index_json.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Deserialization shape for `index.json` as published by the Phase 1 indexer.
//! Kept in sync with `registry/scripts/build_index/src/index_json.rs`. The
//! daemon side does not need every field — those it ignores are skipped via
//! `serde(default)`.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Index {
    pub schema_version: u32,
    pub generated_at: String,
    pub min_client: String,
    pub backends: Vec<IndexBackend>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexBackend {
    pub id: String,
    pub source: String,
    pub version: String,
    pub tag: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub license: String,
    pub kind: String,
    pub contract: String,
    pub entrypoint: String,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    pub online: bool,
    pub supports_gpu: bool,
    pub supports_cpu: bool,
    pub models: Vec<IndexModel>,
    pub secrets: Vec<IndexSecret>,
    pub options: Vec<IndexOption>,
    pub assets: IndexAssets,
    #[serde(default)]
    pub index_stale: Option<IndexStale>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexModel {
    pub name: String, pub provider: String, pub supported_devices: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexSecret {
    pub name: String, pub label: String, pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexOption {
    pub name: String, pub label: String, pub r#type: String,
    #[serde(default)] pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct IndexAssets {
    #[serde(default)] pub wasm: Option<IndexAsset>,
    #[serde(default)] pub subprocess: Vec<IndexSubprocessAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexAsset { pub url: String, pub size: u64, pub sha256: String }

#[derive(Debug, Clone, Deserialize)]
pub struct IndexSubprocessAsset {
    pub target: String, pub accel: String,
    #[serde(default)] pub cuda_major: Option<u32>,
    #[serde(default)] pub cuda_sm: Option<u32>,
    #[serde(default)] pub cudnn: bool,
    pub url: String, pub size: u64, pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexStale {
    pub latest_attempted: String, pub tag: String, pub error: String, pub since: String,
}
```

- [ ] **Step 4: Register the new top-level module**

In `super-stt-daemon/src/lib.rs` (or the equivalent that lists modules), add:

```rust
pub mod registry;
```

- [ ] **Step 5: Build + commit**

```bash
cargo check --workspace
git add super-stt-daemon/
git commit -m "feat(daemon/registry): module + index.json schema"
```

---

### Task 7: Registry client with ETag-aware cache

**Files:**
- Modify: `super-stt-daemon/src/registry/client.rs`
- Test: inline `#[cfg(test)]` with `mockito` (the daemon's existing test scaffolding for `wasi:http` already uses mock HTTP — find the pattern in the daemon's tests dir and reuse)

- [ ] **Step 1: Write the client**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Fetch and cache the registry's `index.json`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::registry::index_schema::Index;

pub const DEFAULT_URL: &str = "https://jorge-menjivar.github.io/super-stt/index.json";
pub const DEFAULT_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("registry unavailable and no cache")]
    Unavailable,
}

#[derive(Clone)]
pub struct Client {
    url: String,
    http: reqwest::Client,
    cache_path: PathBuf,
    ttl: Duration,
    state: Arc<RwLock<Option<Cached>>>,
}

#[derive(Clone)]
struct Cached {
    index: Index,
    etag: Option<String>,
    fetched_at: SystemTime,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    etag: Option<String>,
    fetched_at_secs: u64,
    index: serde_json::Value,
}

impl Client {
    pub fn new(url: impl Into<String>, cache_path: PathBuf, ttl: Duration) -> Self {
        Self {
            url: url.into(),
            http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap(),
            cache_path, ttl,
            state: Arc::default(),
        }
    }

    pub fn from_env() -> Self {
        let url = std::env::var("SUPER_STT_REGISTRY_URL").unwrap_or_else(|_| DEFAULT_URL.into());
        let cache_dir = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("super-stt");
        std::fs::create_dir_all(&cache_dir).ok();
        Self::new(url, cache_dir.join("registry-index.json"), DEFAULT_TTL)
    }

    /// Get the index. Uses memory → file cache → network in that order; falls
    /// back to whichever is freshest if the network is down.
    pub async fn get(&self) -> Result<Index, ClientError> {
        if let Some(c) = self.state.read().as_ref() {
            if c.fetched_at.elapsed().unwrap_or_default() < self.ttl {
                return Ok(c.index.clone());
            }
        }
        // Try a refresh; on failure fall back to whatever's cached.
        match self.refresh().await {
            Ok(idx) => Ok(idx),
            Err(_) => {
                if let Some(c) = self.state.read().as_ref() {
                    return Ok(c.index.clone());
                }
                Err(ClientError::Unavailable)
            }
        }
    }

    /// Force-refresh. Pre-populates the in-memory cache from the on-disk
    /// cache on first call (so the daemon can start cold and still serve
    /// the prior index without a successful network fetch).
    pub async fn refresh(&self) -> Result<Index, ClientError> {
        // Load from disk if memory is empty.
        let etag = {
            let guard = self.state.read();
            match guard.as_ref() {
                Some(c) => c.etag.clone(),
                None => self.load_from_disk()?.and_then(|(idx, etag)| {
                    drop(guard);
                    self.state.write().replace(Cached { index: idx, etag: etag.clone(), fetched_at: SystemTime::UNIX_EPOCH });
                    etag
                }),
            }
        };
        let mut req = self.http.get(&self.url);
        if let Some(e) = &etag { req = req.header("If-None-Match", e); }
        let resp = req.send().await?;
        if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
            // Bump fetched_at to extend TTL.
            if let Some(c) = self.state.write().as_mut() { c.fetched_at = SystemTime::now(); }
            return Ok(self.state.read().as_ref().unwrap().index.clone());
        }
        let resp = resp.error_for_status()?;
        let new_etag = resp.headers().get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok()).map(String::from);
        let bytes = resp.bytes().await?;
        let index: Index = serde_json::from_slice(&bytes)?;
        let cached = Cached { index: index.clone(), etag: new_etag, fetched_at: SystemTime::now() };
        self.state.write().replace(cached.clone());
        self.persist(&cached, &bytes)?;
        Ok(index)
    }

    fn load_from_disk(&self) -> Result<Option<(Index, Option<String>)>, ClientError> {
        if !self.cache_path.exists() { return Ok(None); }
        let bytes = std::fs::read(&self.cache_path)?;
        let file: CacheFile = serde_json::from_slice(&bytes)?;
        let index: Index = serde_json::from_value(file.index)?;
        Ok(Some((index, file.etag)))
    }

    fn persist(&self, c: &Cached, body: &[u8]) -> Result<(), ClientError> {
        let file = CacheFile {
            etag: c.etag.clone(),
            fetched_at_secs: c.fetched_at.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs(),
            index: serde_json::from_slice(body)?,
        };
        let tmp = self.cache_path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(&file)?)?;
        std::fs::rename(tmp, &self.cache_path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture_index() -> &'static str {
        r#"{"schema_version":1,"generated_at":"now","min_client":"0.0.0","backends":[]}"#
    }

    #[tokio::test]
    async fn fetches_and_caches() {
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/idx.json").with_status(200)
            .with_header("etag", "\"abc\"")
            .with_body(fixture_index()).create_async().await;
        let dir = tempdir().unwrap();
        let c = Client::new(format!("{}/idx.json", s.url()), dir.path().join("c.json"), DEFAULT_TTL);
        let idx = c.refresh().await.unwrap();
        assert_eq!(idx.schema_version, 1);
        assert!(dir.path().join("c.json").exists());
    }

    #[tokio::test]
    async fn returns_cache_when_network_fails() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("c.json");
        std::fs::write(&cache_path, format!(
            r#"{{"etag":null,"fetched_at_secs":0,"index":{}}}"#, fixture_index()
        )).unwrap();
        let c = Client::new("http://127.0.0.1:1/never", cache_path, DEFAULT_TTL);
        let idx = c.get().await.unwrap();
        assert_eq!(idx.schema_version, 1);
    }
}
```

- [ ] **Step 2: Build + test + commit**

```bash
cargo test -p super-stt-daemon --lib registry::client
git add super-stt-daemon/
git commit -m "feat(daemon/registry): client with ETag cache + on-disk fallback"
```

---

### Task 8: Host detection

**Files:**
- Modify: `super-stt-daemon/src/registry/host_detect.rs`

- [ ] **Step 1: Check whether the daemon already has GPU detection**

```bash
grep -rn "nvml\|NvmlError\|/sys/class/drm\|amdgpu" super-stt-daemon/src/
```

If yes, reuse via a `pub fn` re-export. If no, add `nvml-wrapper` to `super-stt-daemon/Cargo.toml`:

```bash
cd super-stt-daemon && cargo add nvml-wrapper
```

- [ ] **Step 2: Write `host_detect.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Detect the host's target triple, CUDA compute capability, runtime CUDA
//! major version, and cuDNN presence. Used by `compat::select` to pick a
//! compatible asset, and surfaced in the install failure error path.

#[derive(Debug, Clone)]
pub struct Host {
    pub target_triple: String,
    pub cuda: Option<CudaHost>,
}

#[derive(Debug, Clone)]
pub struct CudaHost {
    pub compute_capability: u32,   // e.g. 86 for sm_86
    pub runtime_major: u32,        // installed CUDA major (12 or 13)
    pub cudnn_present: bool,
}

pub fn detect() -> Host {
    Host {
        target_triple: target_triple().into(),
        cuda: detect_cuda(),
    }
}

/// Compile-time host triple. The daemon binary is built for one target,
/// so the host triple equals the build triple. Hard-fails to compile on
/// platforms the daemon doesn't yet support — intentional: that's a build-
/// time signal that someone needs to add an arm here.
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn target_triple() -> &'static str { "x86_64-unknown-linux-gnu" }

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn target_triple() -> &'static str { "aarch64-unknown-linux-gnu" }

fn detect_cuda() -> Option<CudaHost> {
    // Attempt to open NVML. If it fails for any reason (no driver, no GPU,
    // permission, library not present), return None — the host has no usable
    // CUDA.
    let nvml = nvml_wrapper::Nvml::init().ok()?;
    let dev = nvml.device_by_index(0).ok()?;
    let (major, minor) = dev.cuda_compute_capability().ok()?;
    let cc = (major as u32) * 10 + (minor as u32);
    let cuda_version = nvml.sys_cuda_driver_version().ok()?;    // e.g. 12090
    let runtime_major = (cuda_version / 1000) as u32;
    let cudnn_present = detect_cudnn();
    Some(CudaHost { compute_capability: cc, runtime_major, cudnn_present })
}

fn detect_cudnn() -> bool {
    // Cheap heuristic: probe well-known cuDNN install paths.
    use std::path::Path;
    for p in &[
        "/usr/lib/x86_64-linux-gnu/libcudnn.so",
        "/usr/lib64/libcudnn.so",
        "/usr/local/cuda/lib64/libcudnn.so",
    ] {
        if Path::new(p).exists() { return true; }
    }
    // Last resort: ldconfig parse.
    if let Ok(out) = std::process::Command::new("ldconfig").arg("-p").output() {
        if String::from_utf8_lossy(&out.stdout).contains("libcudnn.so") {
            return true;
        }
    }
    false
}
```

If the daemon needs to support additional platforms (Windows, macOS, other Linux arches), add the corresponding `#[cfg]` arm to `target_triple()`. The conditional compilation here is deliberate: the daemon only supports the platforms it has an arm for.

- [ ] **Step 3: Compile-check**

Run: `cargo check -p super-stt-daemon`
Expected: clean. If `nvml_wrapper` isn't present on the host, the build still succeeds — the detection runtime-checks via `init().ok()?`.

- [ ] **Step 4: Commit**

```bash
git add super-stt-daemon/
git commit -m "feat(daemon/registry): host detection (target triple, CUDA sm, cuDNN)"
```

---

### Task 9: Compatibility selection algorithm

**Files:**
- Modify: `super-stt-daemon/src/registry/compat.rs`

- [ ] **Step 1: Write the pure selection function**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! `select_asset(host, entry, prefs)` — pure: no I/O, no shared state.

use super_stt_shared::registry::SelectedAsset;

use crate::registry::host_detect::Host;
use crate::registry::index_schema::{IndexBackend, IndexSubprocessAsset};

#[derive(Debug, Clone, Default)]
pub struct Prefs {
    /// User-asked to prefer GPU for this backend. Mirrors today's per-local-
    /// model "Use GPU" checkbox.
    pub prefer_gpu: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Wasm,
    Subprocess { index: usize },
    Incompatible { reason: String },
}

pub fn select(host: &Host, entry: &IndexBackend, prefs: &Prefs) -> Selection {
    if entry.kind == "wasm" {
        return if entry.assets.wasm.is_some() {
            Selection::Wasm
        } else {
            Selection::Incompatible { reason: "wasm backend missing wasm asset".into() }
        };
    }
    if entry.kind != "subprocess" {
        return Selection::Incompatible { reason: format!("unknown kind `{}`", entry.kind) };
    }
    // Filter by target triple.
    let by_target: Vec<(usize, &IndexSubprocessAsset)> = entry.assets.subprocess.iter().enumerate()
        .filter(|(_, a)| a.target == host.target_triple).collect();
    if by_target.is_empty() {
        return Selection::Incompatible {
            reason: format!("no asset for target `{}`", host.target_triple),
        };
    }

    if prefs.prefer_gpu {
        if let Some(cuda) = &host.cuda {
            let cuda_matches: Vec<&(usize, &IndexSubprocessAsset)> = by_target.iter()
                .filter(|(_, a)| a.accel == "cuda"
                    && a.cuda_sm == Some(cuda.compute_capability)
                    && a.cuda_major.map_or(false, |m| m <= cuda.runtime_major)
                ).collect();
            // Preference: highest cuda_major; cudnn if host has it.
            let best = cuda_matches.iter().max_by_key(|(_, a)| {
                (
                    a.cuda_major.unwrap_or(0),
                    (a.cudnn && cuda.cudnn_present) as u8,
                )
            });
            if let Some(&&(idx, _)) = best {
                return Selection::Subprocess { index: idx };
            }
            // Fall through to CPU.
        }
    }
    // CPU fallback.
    if let Some((idx, _)) = by_target.iter().find(|(_, a)| a.accel == "cpu") {
        return Selection::Subprocess { index: *idx };
    }
    Selection::Incompatible {
        reason: format!("no compatible asset for host `{}`, sm_{}",
            host.target_triple,
            host.cuda.as_ref().map(|c| c.compute_capability.to_string()).unwrap_or("?".into())),
    }
}

pub fn to_selected_asset(entry: &IndexBackend, sel: &Selection) -> Option<SelectedAsset> {
    match sel {
        Selection::Wasm => entry.assets.wasm.as_ref().map(|_| SelectedAsset {
            target: String::new(), accel: "wasm".into(),
            cuda_major: None, cuda_sm: None, cudnn: false,
        }),
        Selection::Subprocess { index } => entry.assets.subprocess.get(*index).map(|a| SelectedAsset {
            target: a.target.clone(), accel: a.accel.clone(),
            cuda_major: a.cuda_major, cuda_sm: a.cuda_sm, cudnn: a.cudnn,
        }),
        Selection::Incompatible { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::host_detect::{Host, CudaHost};
    use crate::registry::index_schema::*;

    fn entry(kind: &str, subprocess: Vec<IndexSubprocessAsset>) -> IndexBackend {
        IndexBackend {
            id: "t".into(), source: "x".into(), version: "1.0.0".into(), tag: "v1.0.0".into(),
            name: "T".into(), description: None, license: "Apache-2.0".into(),
            kind: kind.into(), contract: "v1".into(), entrypoint: "t".into(),
            allowed_hosts: vec![], online: false, supports_gpu: true, supports_cpu: true,
            models: vec![], secrets: vec![], options: vec![],
            assets: IndexAssets { wasm: None, subprocess },
            index_stale: None,
        }
    }

    fn sp(target: &str, accel: &str, sm: Option<u32>, cm: Option<u32>, cudnn: bool) -> IndexSubprocessAsset {
        IndexSubprocessAsset {
            target: target.into(), accel: accel.into(),
            cuda_major: cm, cuda_sm: sm, cudnn,
            url: "x".into(), size: 1, sha256: "x".into(),
        }
    }

    fn host_cuda(sm: u32, cm: u32, cudnn: bool) -> Host {
        Host {
            target_triple: "x86_64-unknown-linux-gnu".into(),
            cuda: Some(CudaHost { compute_capability: sm, runtime_major: cm, cudnn_present: cudnn }),
        }
    }

    #[test]
    fn picks_matching_cuda_when_gpu_preferred() {
        let e = entry("subprocess", vec![
            sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
            sp("x86_64-unknown-linux-gnu", "cuda", Some(86), Some(12), false),
            sp("x86_64-unknown-linux-gnu", "cuda", Some(90), Some(12), false),
        ]);
        let sel = select(&host_cuda(86, 12, false), &e, &Prefs { prefer_gpu: true });
        assert_eq!(sel, Selection::Subprocess { index: 1 });
    }

    #[test]
    fn falls_back_to_cpu_when_no_sm_match() {
        let e = entry("subprocess", vec![
            sp("x86_64-unknown-linux-gnu", "cpu", None, None, false),
            sp("x86_64-unknown-linux-gnu", "cuda", Some(90), Some(12), false),
        ]);
        let sel = select(&host_cuda(86, 12, false), &e, &Prefs { prefer_gpu: true });
        assert_eq!(sel, Selection::Subprocess { index: 0 });
    }

    #[test]
    fn prefers_cudnn_when_host_has_it() {
        let e = entry("subprocess", vec![
            sp("x86_64-unknown-linux-gnu", "cuda", Some(86), Some(12), false),
            sp("x86_64-unknown-linux-gnu", "cuda", Some(86), Some(12), true),
        ]);
        let sel = select(&host_cuda(86, 12, true), &e, &Prefs { prefer_gpu: true });
        assert_eq!(sel, Selection::Subprocess { index: 1 });
    }

    #[test]
    fn picks_highest_cuda_major_within_runtime() {
        let e = entry("subprocess", vec![
            sp("x86_64-unknown-linux-gnu", "cuda", Some(86), Some(12), false),
            sp("x86_64-unknown-linux-gnu", "cuda", Some(86), Some(13), false),
        ]);
        // Host has CUDA 13 runtime
        let sel = select(&host_cuda(86, 13, false), &e, &Prefs { prefer_gpu: true });
        assert_eq!(sel, Selection::Subprocess { index: 1 });
    }

    #[test]
    fn cuda_runtime_caps_choice() {
        let e = entry("subprocess", vec![
            sp("x86_64-unknown-linux-gnu", "cuda", Some(86), Some(12), false),
            sp("x86_64-unknown-linux-gnu", "cuda", Some(86), Some(13), false),
        ]);
        // Host has CUDA 12 runtime — must not pick the cuda_major=13 build.
        let sel = select(&host_cuda(86, 12, false), &e, &Prefs { prefer_gpu: true });
        assert_eq!(sel, Selection::Subprocess { index: 0 });
    }

    #[test]
    fn target_mismatch_is_incompatible() {
        let e = entry("subprocess", vec![sp("aarch64-unknown-linux-gnu", "cpu", None, None, false)]);
        let sel = select(&host_cuda(86, 12, false), &e, &Prefs { prefer_gpu: false });
        assert!(matches!(sel, Selection::Incompatible { .. }));
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p super-stt-daemon --lib registry::compat
```
Expected: all 6 tests pass.

- [ ] **Step 3: Commit**

```bash
git add super-stt-daemon/
git commit -m "feat(daemon/registry): compatibility selection algorithm + tests"
```

---

### Task 10: Install pipeline state machine

**Files:**
- Modify: `super-stt-daemon/src/registry/install.rs`

- [ ] **Step 1: Add `tar`, `flate2`, `hex` to daemon deps**

```bash
cd super-stt-daemon && cargo add tar flate2 hex
```

- [ ] **Step 2: Write `install.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Install pipeline. State machine: Resolving → Downloading → Verifying →
//! Extracting → Installing → Rescanning → Done | Failed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use ring::digest::{Context, SHA256};
use super_stt_shared::registry::events::{InstallError, InstallPhase};
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::registry::compat::{Selection};
use crate::registry::index_schema::{IndexBackend, IndexSubprocessAsset};

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("hash mismatch: expected `{expected}`, got `{actual}`")]
    HashMismatch { expected: String, actual: String },
    #[error("tarball contains unsafe entry: {0}")]
    TarUnsafe(String),
}

impl PipelineError {
    pub fn as_typed(&self, phase: InstallPhase) -> (InstallPhase, InstallError) {
        match self {
            PipelineError::Network(_) => (phase, InstallError::DownloadFailed),
            PipelineError::HashMismatch { .. } => (InstallPhase::Verifying, InstallError::AssetHashMismatch),
            PipelineError::TarUnsafe(_) => (InstallPhase::Extracting, InstallError::TarballUnsafe),
            PipelineError::Io(_) => (phase, InstallError::InstallIoError),
        }
    }
}

pub struct Pipeline<F> {
    pub backends_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub http: reqwest::Client,
    pub on_progress: Arc<F>,
}

/// Run an install. `entry` and `selection` come from the registry client +
/// compat module. Returns the installed version on success.
pub async fn run<F>(p: &Pipeline<F>, entry: &IndexBackend, selection: &Selection) -> Result<String, (InstallPhase, InstallError)>
where F: Fn(InstallPhase, Option<(u64, Option<u64>)>) + Send + Sync {
    use InstallPhase as P;

    (p.on_progress)(P::Resolving, None);

    let (url, expected_sha, kind_subdir) = match selection {
        Selection::Wasm => {
            let a = entry.assets.wasm.as_ref().expect("wasm selection only when present");
            (a.url.clone(), a.sha256.clone(), false)
        }
        Selection::Subprocess { index } => {
            let a: &IndexSubprocessAsset = &entry.assets.subprocess[*index];
            (a.url.clone(), a.sha256.clone(), true)
        }
        Selection::Incompatible { reason: _ } => {
            return Err((P::Resolving, InstallError::Incompatible));
        }
    };

    let partial_name = format!("{}-{}.{}.partial", entry.id, entry.version,
        if kind_subdir { "tar.gz" } else { "wasm" });
    let partial_path = p.cache_dir.join(&partial_name);
    fs::create_dir_all(&p.cache_dir).await.map_err(|e| {
        PipelineError::Io(e).as_typed(P::Downloading)
    })?;

    // Download + hash.
    (p.on_progress)(P::Downloading, Some((0, None)));
    let actual_sha = stream_download(&p.http, &url, &partial_path, &p.on_progress).await
        .map_err(|e| e.as_typed(P::Downloading))?;

    // Verify.
    (p.on_progress)(P::Verifying, None);
    if actual_sha != expected_sha {
        let _ = fs::remove_file(&partial_path).await;
        return Err((P::Verifying, InstallError::AssetHashMismatch));
    }

    // Stage + extract.
    (p.on_progress)(P::Extracting, None);
    let staging = p.backends_dir.join(".staging").join(format!("{}-{}", entry.id, entry.version));
    if staging.exists() { fs::remove_dir_all(&staging).await.ok(); }
    fs::create_dir_all(&staging).await.map_err(|e| PipelineError::Io(e).as_typed(P::Extracting))?;
    if kind_subdir {
        extract_tarball(&partial_path, &staging).map_err(|e| e.as_typed(P::Extracting))?;
    } else {
        // Wasm: copy the file into the staging dir as <entrypoint>.
        let dest = staging.join(&entry.entrypoint);
        fs::copy(&partial_path, &dest).await.map_err(|e| PipelineError::Io(e).as_typed(P::Extracting))?;
    }

    // Write the index-recorded backend.toml. We do not trust whatever may
    // have been packed inside the tarball.
    let toml_text = synthesize_backend_toml(entry);
    fs::write(staging.join("backend.toml"), toml_text).await
        .map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;

    // Atomic rename into final location.
    (p.on_progress)(P::Installing, None);
    let final_path = p.backends_dir.join(&entry.id);
    if final_path.exists() { fs::remove_dir_all(&final_path).await.ok(); }
    fs::rename(&staging, &final_path).await.map_err(|e| PipelineError::Io(e).as_typed(P::Installing))?;
    let _ = fs::remove_file(&partial_path).await;

    (p.on_progress)(P::Rescanning, None);
    Ok(entry.version.clone())
}

async fn stream_download<F>(http: &reqwest::Client, url: &str, dest: &Path, on_progress: &Arc<F>)
-> Result<String, PipelineError>
where F: Fn(InstallPhase, Option<(u64, Option<u64>)>) + Send + Sync {
    let resp = http.get(url).send().await?.error_for_status()?;
    let total = resp.content_length();
    let mut file = tokio::fs::File::create(dest).await?;
    let mut hasher = Context::new(&SHA256);
    let mut stream = resp.bytes_stream();
    let mut bytes_done: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        hasher.update(&chunk);
        bytes_done += chunk.len() as u64;
        on_progress(InstallPhase::Downloading, Some((bytes_done, total)));
    }
    file.flush().await?;
    Ok(hex::encode(hasher.finish().as_ref()))
}

fn extract_tarball(src: &Path, dest_dir: &Path) -> Result<(), PipelineError> {
    let f = std::fs::File::open(src)?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(true);
    archive.set_overwrite(true);
    // Validate before extracting.
    {
        let f2 = std::fs::File::open(src)?;
        let gz2 = flate2::read::GzDecoder::new(f2);
        let mut a2 = tar::Archive::new(gz2);
        for entry in a2.entries()? {
            let entry = entry?;
            let path = entry.path()?;
            let s = path.to_string_lossy();
            if s.starts_with('/') || s.contains("..") {
                return Err(PipelineError::TarUnsafe(s.into()));
            }
            if entry.header().entry_type().is_symlink() {
                return Err(PipelineError::TarUnsafe(format!("symlink: {s}")));
            }
        }
    }
    archive.unpack(dest_dir)?;
    Ok(())
}

fn synthesize_backend_toml(entry: &IndexBackend) -> String {
    // Reconstruct a minimal backend.toml that matches what the daemon's
    // `backends::discover` expects today. The runtime fields (source/name/
    // version/kind/entrypoint/contract + secrets/options/models) are what
    // discover reads — `[assets]` is not needed at runtime.
    let mut out = String::new();
    out.push_str("# SPDX-License-Identifier: GPL-3.0-only\n");
    out.push_str("# Synthesized by daemon's registry installer from index.json.\n\n");
    out.push_str("[backend]\n");
    out.push_str(&format!("source = \"{}\"\n", entry.source));
    out.push_str(&format!("name = \"{}\"\n", entry.name));
    out.push_str(&format!("version = \"{}\"\n", entry.version));
    out.push_str(&format!("kind = \"{}\"\n", entry.kind));
    out.push_str(&format!("entrypoint = \"{}\"\n", entry.entrypoint));
    out.push_str(&format!("contract = \"{}\"\n", entry.contract));
    if !entry.license.is_empty() {
        out.push_str(&format!("license = \"{}\"\n", entry.license));
    }
    if !entry.allowed_hosts.is_empty() {
        out.push_str("\n[network]\n");
        let hosts: Vec<String> = entry.allowed_hosts.iter().map(|h| format!("\"{h}\"")).collect();
        out.push_str(&format!("allowed_hosts = [{}]\n", hosts.join(", ")));
    }
    for s in &entry.secrets {
        out.push_str("\n[[secrets]]\n");
        out.push_str(&format!("name = \"{}\"\n", s.name));
        out.push_str(&format!("label = \"{}\"\n", s.label));
        out.push_str(&format!("required = {}\n", s.required));
    }
    for o in &entry.options {
        out.push_str("\n[[options]]\n");
        out.push_str(&format!("name = \"{}\"\n", o.name));
        out.push_str(&format!("label = \"{}\"\n", o.label));
        out.push_str(&format!("type = \"{}\"\n", o.r#type));
        if let Some(d) = &o.default {
            out.push_str(&format!("default = {}\n", serde_json::to_string(d).unwrap_or_default()));
        }
    }
    for md in &entry.models {
        out.push_str("\n[[models]]\n");
        out.push_str(&format!("name = \"{}\"\n", md.name));
        out.push_str(&format!("provider = \"{}\"\n", md.provider));
        let devs: Vec<String> = md.supported_devices.iter().map(|d| format!("\"{d}\"")).collect();
        out.push_str(&format!("supported_devices = [{}]\n", devs.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesizes_minimal_backend_toml() {
        let entry = IndexBackend {
            id: "openai".into(), source: "github.com/x/y".into(),
            version: "1.0.0".into(), tag: "v1.0.0".into(),
            name: "OpenAI".into(), description: None, license: "Apache-2.0".into(),
            kind: "wasm".into(), contract: "v1".into(), entrypoint: "openai.wasm".into(),
            allowed_hosts: vec!["api.openai.com".into()],
            online: true, supports_gpu: false, supports_cpu: false,
            models: vec![], secrets: vec![], options: vec![],
            assets: crate::registry::index_schema::IndexAssets::default(),
            index_stale: None,
        };
        let s = synthesize_backend_toml(&entry);
        assert!(s.contains("source = \"github.com/x/y\""));
        assert!(s.contains("kind = \"wasm\""));
        assert!(s.contains("api.openai.com"));
    }
}
```

- [ ] **Step 3: Build + test + commit**

```bash
cargo test -p super-stt-daemon --lib registry::install
git add super-stt-daemon/
git commit -m "feat(daemon/registry): install pipeline + tar safety + sha verify"
```

---

### Task 11: Wire `GET /registry/backends` handler

**Files:**
- Modify: `super-stt-daemon/src/daemon/http_server.rs`
- Modify: `super-stt-daemon/src/daemon/handlers.rs`

- [ ] **Step 1: Add the handler in `handlers.rs`**

Open `handlers.rs` and add (near the other backend-related handlers):

```rust
use super_stt_shared::registry::{
    Compatibility, RegistryBackend, RegistryListResponse, RegistryModel,
    RegistryOption, RegistrySecret,
};

#[derive(serde::Deserialize, Default)]
pub struct ListRegistryQuery {
    #[serde(default)]
    pub include_incompatible: bool,
    pub kind: Option<String>,
    pub online: Option<bool>,
    pub q: Option<String>,
}

pub async fn list_registry_backends(
    State(state): State<AppState>,
    Query(q): Query<ListRegistryQuery>,
) -> Result<Json<RegistryListResponse>, ApiError> {
    let index = state.registry_client.get().await
        .map_err(|_| ApiError::ServiceUnavailable("registry_unavailable"))?;

    let host = crate::registry::host_detect::detect();
    let installed = state.installed_backend_versions().await;

    let mut out = Vec::with_capacity(index.backends.len());
    for entry in &index.backends {
        if let Some(k) = &q.kind { if &entry.kind != k { continue; } }
        if let Some(online_q) = q.online { if entry.online != online_q { continue; } }
        if let Some(needle) = &q.q {
            let needle = needle.to_lowercase();
            let hay = format!("{} {}",
                entry.name.to_lowercase(),
                entry.description.as_deref().unwrap_or("").to_lowercase());
            if !hay.contains(&needle) { continue; }
        }

        let prefs = crate::registry::compat::Prefs::default();
        let sel = crate::registry::compat::select(&host, entry, &prefs);
        let compat = match &sel {
            crate::registry::compat::Selection::Incompatible { reason } => {
                if !q.include_incompatible { continue; }
                Compatibility { compatible: false, selected_asset: None, reason: Some(reason.clone()) }
            }
            _ => Compatibility {
                compatible: true,
                selected_asset: crate::registry::compat::to_selected_asset(entry, &sel),
                reason: None,
            },
        };

        out.push(RegistryBackend {
            id: entry.id.clone(),
            source: entry.source.clone(),
            version: entry.version.clone(),
            name: entry.name.clone(),
            description: entry.description.clone(),
            license: entry.license.clone(),
            kind: entry.kind.clone(),
            contract: entry.contract.clone(),
            allowed_hosts: entry.allowed_hosts.clone(),
            online: entry.online,
            supports_gpu: entry.supports_gpu,
            supports_cpu: entry.supports_cpu,
            models: entry.models.iter().map(|m| RegistryModel {
                name: m.name.clone(), provider: m.provider.clone(),
                supported_devices: m.supported_devices.clone(),
            }).collect(),
            secrets: entry.secrets.iter().map(|s| RegistrySecret {
                name: s.name.clone(), label: s.label.clone(), required: s.required,
            }).collect(),
            options: entry.options.iter().map(|o| RegistryOption {
                name: o.name.clone(), label: o.label.clone(),
                r#type: o.r#type.clone(), default: o.default.clone(),
            }).collect(),
            compatibility: compat,
            installed_version: installed.get(&entry.source).cloned(),
            index_stale: entry.index_stale.as_ref().map(|s| super_stt_shared::registry::IndexStale {
                latest_attempted: s.latest_attempted.clone(),
                tag: s.tag.clone(), error: s.error.clone(), since: s.since.clone(),
            }),
        });
    }

    Ok(Json(RegistryListResponse {
        schema_version: index.schema_version,
        generated_at: index.generated_at,
        backends: out,
    }))
}
```

You will need to add `Query` to the imports, and `installed_backend_versions()` to `AppState` — see Step 2.

- [ ] **Step 2: Add `installed_backend_versions()` to AppState and add the registry client to AppState**

Open the AppState definition (search `pub struct AppState`). Add:

```rust
pub registry_client: Arc<crate::registry::client::Client>,
```

And initialize it where `AppState` is constructed (search for where the existing state is built):

```rust
registry_client: Arc::new(crate::registry::client::Client::from_env()),
```

Add an inherent method to AppState (in the same file) that walks the discovered backends and returns a `HashMap<source, version>`:

```rust
impl AppState {
    pub async fn installed_backend_versions(&self) -> std::collections::HashMap<String, String> {
        // The daemon's discover() reads `backend.toml`s from the install dir;
        // each one's `[backend].source` + `version` are the keys we want.
        let dir = dirs::data_dir().unwrap_or_default().join("super-stt").join("backends");
        let discovered = crate::stt_models::backends::discover(&dir);
        discovered.into_iter()
            .map(|b| (b.config.backend.source.clone(), b.config.backend.version.clone()))
            .collect()
    }
}
```

Adjust field names to match the actual `DiscoveredBackend` struct in the daemon's `stt_models/backends/` (look at how the existing `GET /backends` handler reads versions).

- [ ] **Step 3: Add a typed `ApiError` variant if absent**

If `ApiError` doesn't have `ServiceUnavailable`, add it:

```rust
ServiceUnavailable(&'static str),
```

and map it to `503` in the `IntoResponse` impl.

- [ ] **Step 4: Register the route**

In `http_server.rs`, near the existing `.route("/backends", get(list_backends))`:

```rust
.route("/registry/backends", get(list_registry_backends))
```

- [ ] **Step 5: Build + smoke-test**

```bash
cargo build -p super-stt-daemon
```

Run the daemon (your usual command) and:

```bash
curl -s http://localhost:<daemon-port>/registry/backends | jq
```
Expected: `{ "schema_version": 1, "generated_at": "…", "backends": [...] }`. Will list whatever Phase 1's index has — 3 entries seeded.

- [ ] **Step 6: Commit**

```bash
git add super-stt-daemon/
git commit -m "feat(daemon/http): GET /registry/backends"
```

---

### Task 12: Wire `POST /registry/backends/refresh`

**Files:**
- Modify: `super-stt-daemon/src/daemon/http_server.rs`
- Modify: `super-stt-daemon/src/daemon/handlers.rs`

- [ ] **Step 1: Add the handler**

```rust
use super_stt_shared::registry::RefreshResponse;

pub async fn refresh_registry(
    State(state): State<AppState>,
) -> Result<Json<RefreshResponse>, ApiError> {
    let index = state.registry_client.refresh().await
        .map_err(|_| ApiError::ServiceUnavailable("registry_unavailable"))?;
    Ok(Json(RefreshResponse {
        schema_version: index.schema_version,
        generated_at: index.generated_at,
        backend_count: index.backends.len(),
    }))
}
```

- [ ] **Step 2: Register the route**

```rust
.route("/registry/backends/refresh", post(refresh_registry))
```

- [ ] **Step 3: Smoke + commit**

```bash
curl -s -X POST http://localhost:<port>/registry/backends/refresh | jq
```
Expected: `{ "schema_version": 1, "generated_at": "…", "backend_count": 3 }`.

```bash
git add super-stt-daemon/
git commit -m "feat(daemon/http): POST /registry/backends/refresh"
```

---

### Task 13: Wire `POST /registry/backends/install` + event emission

**Files:**
- Modify: `super-stt-daemon/src/daemon/http_server.rs`
- Modify: `super-stt-daemon/src/daemon/handlers.rs`
- Modify: `super-stt-daemon/src/daemon/events.rs` (or wherever event emission lives — find via `grep -rn "fn emit\|broadcast::Sender" super-stt-daemon/src/daemon/`)

- [ ] **Step 1: Add the handler**

```rust
use super_stt_shared::registry::{InstallAccepted, InstallRequest};
use super_stt_shared::registry::events::{InstallPhase, RegistryEvent};

pub async fn install_registry_backend(
    State(state): State<AppState>,
    Json(req): Json<InstallRequest>,
) -> Result<(axum::http::StatusCode, Json<InstallAccepted>), ApiError> {
    let (source, custom_repo) = match req {
        InstallRequest::BySource { source } => (source, false),
        InstallRequest::ByRepoUrl { repo_url } => (repo_url, true),
    };

    // Coalesce concurrent installs for the same source.
    if !state.install_inflight.write().insert(source.clone()) {
        return Err(ApiError::Conflict("install_in_progress"));
    }

    let entry = if custom_repo {
        // Custom-repo path: not part of Phase 2 MVP. Return 400 for now.
        state.install_inflight.write().remove(&source);
        return Err(ApiError::BadRequest("custom_repo_not_yet_supported"));
    } else {
        let index = state.registry_client.get().await
            .map_err(|_| ApiError::ServiceUnavailable("registry_unavailable"))?;
        index.backends.iter().find(|b| b.source == source).cloned()
            .ok_or_else(|| ApiError::NotFound("source_not_in_registry"))?
    };

    let host = crate::registry::host_detect::detect();
    let prefs = crate::registry::compat::Prefs::default();
    let sel = crate::registry::compat::select(&host, &entry, &prefs);
    if matches!(sel, crate::registry::compat::Selection::Incompatible { .. }) {
        state.install_inflight.write().remove(&source);
        return Err(ApiError::BadRequest("incompatible"));
    }

    let install_id = format!("ins_{}", ulid::Ulid::new());
    let selected = crate::registry::compat::to_selected_asset(&entry, &sel)
        .expect("compatible selection has an asset");
    let accepted = InstallAccepted {
        install_id: install_id.clone(),
        source: source.clone(),
        version: entry.version.clone(),
        selected_asset: selected,
        warning: None,
    };

    // Spawn background task.
    let tx = state.event_tx.clone();
    let backends_dir = dirs::data_dir().unwrap_or_default().join("super-stt").join("backends");
    let cache_dir = dirs::cache_dir().unwrap_or_default().join("super-stt").join("downloads");
    let inflight = state.install_inflight.clone();
    let source_owned = source.clone();
    let install_id_owned = install_id.clone();
    let entry_owned = entry.clone();
    let sel_owned = sel.clone();

    tokio::spawn(async move {
        let tx2 = tx.clone();
        let iid = install_id_owned.clone();
        let src = source_owned.clone();
        let pipeline = crate::registry::install::Pipeline {
            backends_dir, cache_dir,
            http: reqwest::Client::new(),
            on_progress: std::sync::Arc::new(move |phase, byteinfo| {
                let evt = RegistryEvent::Progress {
                    install_id: iid.clone(),
                    source: src.clone(),
                    phase,
                    bytes_done: byteinfo.map(|(d, _)| d),
                    bytes_total: byteinfo.and_then(|(_, t)| t),
                };
                let _ = tx2.send(serde_json::to_string(&evt).unwrap_or_default());
            }),
        };
        let result = crate::registry::install::run(&pipeline, &entry_owned, &sel_owned).await;
        let outcome = match result {
            Ok(v) => RegistryEvent::Completed { install_id: install_id_owned.clone(), source: source_owned.clone(), version: v },
            Err((phase, err)) => RegistryEvent::Failed { install_id: install_id_owned.clone(), source: source_owned.clone(), phase, error: err },
        };
        let _ = tx.send(serde_json::to_string(&outcome).unwrap_or_default());

        // Refresh discovery after install.
        // (If your daemon caches the discovered list, refresh it here.)

        inflight.write().remove(&source_owned);
    });

    Ok((axum::http::StatusCode::ACCEPTED, Json(accepted)))
}
```

You will need:
- `install_inflight: Arc<RwLock<HashSet<String>>>` on AppState.
- `event_tx: tokio::sync::broadcast::Sender<String>` on AppState (find the existing one used by `/events`).
- `ulid` added: `cargo add ulid`.
- `ApiError::Conflict`, `BadRequest`, `NotFound` variants — add if missing.

- [ ] **Step 2: Register the route**

```rust
.route("/registry/backends/install", post(install_registry_backend))
```

- [ ] **Step 3: Smoke-test**

```bash
# Stream events in one terminal
curl -N http://localhost:<port>/events &
# Install in another
curl -s -X POST http://localhost:<port>/registry/backends/install \
  -H 'content-type: application/json' \
  -d '{"source":"github.com/jorge-menjivar/super-stt"}'
```
Expected: 202 with `install_id`, then events stream `progress.downloading` → `verifying` → `extracting` → `installing` → `rescanning` → `completed`.

- [ ] **Step 4: Commit**

```bash
git add super-stt-daemon/
git commit -m "feat(daemon/http): POST /registry/backends/install + events"
```

---

### Task 14: Wire `POST /registry/backends/update`

**Files:**
- Modify: `super-stt-daemon/src/daemon/http_server.rs`
- Modify: `super-stt-daemon/src/daemon/handlers.rs`

- [ ] **Step 1: Add the handler**

```rust
use super_stt_shared::registry::{UpdateRequest, UpdateResponse};

pub async fn update_registry_backend(
    State(state): State<AppState>,
    Json(req): Json<UpdateRequest>,
) -> Result<Json<UpdateResponse>, ApiError> {
    let installed = state.installed_backend_versions().await;
    let from_version = installed.get(&req.source).cloned()
        .ok_or_else(|| ApiError::NotFound("source_not_installed"))?;

    let index = state.registry_client.get().await
        .map_err(|_| ApiError::ServiceUnavailable("registry_unavailable"))?;
    let entry = index.backends.iter().find(|b| b.source == req.source).cloned()
        .ok_or_else(|| ApiError::NotFound("source_not_in_registry"))?;

    if entry.version == from_version {
        return Ok(Json(UpdateResponse {
            install_id: None,
            from_version: from_version.clone(),
            to_version: from_version,
            noop: true,
        }));
    }

    // Delegate to the install path; it overwrites in place.
    let install_req = super_stt_shared::registry::InstallRequest::BySource { source: req.source };
    let (_, Json(accepted)) = install_registry_backend(State(state), Json(install_req)).await?;
    Ok(Json(UpdateResponse {
        install_id: Some(accepted.install_id),
        from_version,
        to_version: accepted.version,
        noop: false,
    }))
}
```

- [ ] **Step 2: Register the route**

```rust
.route("/registry/backends/update", post(update_registry_backend))
```

- [ ] **Step 3: Smoke + commit**

```bash
curl -s -X POST http://localhost:<port>/registry/backends/update \
  -H 'content-type: application/json' \
  -d '{"source":"github.com/jorge-menjivar/super-stt"}'
```

Expected: `noop: true` when already current; otherwise an `install_id` and events follow.

```bash
git add super-stt-daemon/
git commit -m "feat(daemon/http): POST /registry/backends/update"
```

---

### Task 15: Wire `DELETE /backends/{source}`

**Files:**
- Modify: `super-stt-daemon/src/daemon/http_server.rs`
- Modify: `super-stt-daemon/src/daemon/handlers.rs`

- [ ] **Step 1: Add the handler**

```rust
use axum::extract::Path;
use super_stt_shared::registry::UninstallResponse;

pub async fn uninstall_backend(
    State(state): State<AppState>,
    Path(source): Path<String>,
) -> Result<Json<UninstallResponse>, ApiError> {
    // Find the backend dir whose backend.toml has this source.
    let backends_dir = dirs::data_dir().unwrap_or_default().join("super-stt").join("backends");
    let discovered = crate::stt_models::backends::discover(&backends_dir);

    let Some(target) = discovered.into_iter().find(|b| b.config.backend.source == source) else {
        return Err(ApiError::NotFound("not_installed"));
    };

    // If this is the active backend, clear it first.
    let was_active = state.active_backend_source().await.as_deref() == Some(&source);
    if was_active {
        state.clear_active_backend().await;
    }

    let dir_to_remove = target.dir.clone(); // adjust to the actual field name on DiscoveredBackend
    tokio::fs::remove_dir_all(&dir_to_remove).await
        .map_err(|_| ApiError::Internal("io_error"))?;

    // Re-trigger discovery so the in-memory catalog reflects the removal.
    state.rescan_backends().await;

    Ok(Json(UninstallResponse { uninstalled: true, was_active }))
}
```

`rescan_backends` is a new helper — implement on AppState as a thin wrapper over whatever today calls `backends::discover` after loading.

- [ ] **Step 2: Register the route**

```rust
.route("/backends/:source", delete(uninstall_backend))
```

- [ ] **Step 3: Smoke + commit**

```bash
curl -s -X DELETE 'http://localhost:<port>/backends/github.com%2Fjorge-menjivar%2Fsuper-stt'
```
Expected: `{"uninstalled":true,"was_active":false}`. The backend's dir is gone from `<XDG_DATA_HOME>/super-stt/backends/`.

```bash
git add super-stt-daemon/
git commit -m "feat(daemon/http): DELETE /backends/{source}"
```

---

### Task 16: HTTP integration tests for the new endpoints

**Files:**
- Create: `super-stt-daemon/tests/registry_http.rs`

- [ ] **Step 1: Write the integration test**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! HTTP integration tests for `/registry/*`. Daemon tests must run via
//! `cargo test --lib` in automated shells because a locked keyring will
//! deadlock the HTTP integration entry points.

use mockito::Server;
use super_stt_shared::registry::{InstallAccepted, RegistryListResponse};

/// Spin up a mock GitHub Pages server hosting a known-good index, point the
/// daemon at it, and verify `GET /registry/backends` returns the expected
/// shape with `compatibility` populated.
#[tokio::test]
#[ignore = "requires daemon HTTP server scaffolding; opt in with --ignored"]
async fn list_registry_backends_returns_compat() {
    let mut idx_srv = Server::new_async().await;
    let idx_body = r#"{
        "schema_version": 1,
        "generated_at": "2026-05-29T18:00:00Z",
        "min_client": "0.0.0",
        "backends": [{
            "id": "openai", "source": "github.com/x/y",
            "version": "1.0.0", "tag": "v1.0.0", "name": "OpenAI",
            "license": "Apache-2.0", "kind": "wasm",
            "contract": "v1", "entrypoint": "openai.wasm",
            "allowed_hosts": ["api.openai.com"],
            "online": true, "supports_gpu": false, "supports_cpu": false,
            "models": [], "secrets": [], "options": [],
            "assets": { "wasm": { "url": "https://example/openai.wasm", "size": 4, "sha256": "abc" } }
        }]
    }"#;
    idx_srv.mock("GET", "/index.json").with_status(200).with_body(idx_body)
        .create_async().await;

    // Point the daemon at the mock and start it. The daemon's existing test
    // harness (see `super-stt-daemon/tests/` for the pattern) is what spins
    // up an in-process server bound to a random port; reuse that here.
    std::env::set_var("SUPER_STT_REGISTRY_URL", format!("{}/index.json", idx_srv.url()));
    let port = start_test_daemon().await;

    let resp: RegistryListResponse = reqwest::get(format!("http://127.0.0.1:{port}/registry/backends"))
        .await.unwrap().json().await.unwrap();
    assert_eq!(resp.backends.len(), 1);
    assert_eq!(resp.backends[0].id, "openai");
    assert!(resp.backends[0].compatibility.compatible);    // wasm = always compatible
}

#[tokio::test]
#[ignore]
async fn install_pipeline_rejects_hash_mismatch() {
    let mut idx_srv = Server::new_async().await;
    let mut asset_srv = Server::new_async().await;

    let idx_body = format!(r#"{{
        "schema_version": 1, "generated_at": "now", "min_client": "0.0.0",
        "backends": [{{
            "id": "openai", "source": "github.com/x/y",
            "version": "1.0.0", "tag": "v1.0.0", "name": "OpenAI",
            "license": "Apache-2.0", "kind": "wasm",
            "contract": "v1", "entrypoint": "openai.wasm",
            "allowed_hosts": [], "online": true, "supports_gpu": false, "supports_cpu": false,
            "models": [], "secrets": [], "options": [],
            "assets": {{ "wasm": {{ "url": "{}/openai.wasm", "size": 4, "sha256": "deadbeef" }} }}
        }}]
    }}"#, asset_srv.url());
    idx_srv.mock("GET", "/index.json").with_status(200).with_body(&idx_body).create_async().await;
    asset_srv.mock("GET", "/openai.wasm").with_status(200)
        .with_body([0x00, 0x61, 0x73, 0x6d].as_slice()).create_async().await;

    std::env::set_var("SUPER_STT_REGISTRY_URL", format!("{}/index.json", idx_srv.url()));
    let port = start_test_daemon().await;

    let accepted: InstallAccepted = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/registry/backends/install"))
        .json(&serde_json::json!({"source":"github.com/x/y"}))
        .send().await.unwrap().json().await.unwrap();

    // Wait for the failure event on /events.
    let resp = reqwest::get(format!("http://127.0.0.1:{port}/events")).await.unwrap();
    // The mock body's hash is sha256("\0asm"), not "deadbeef" — installer
    // must emit registry.install.failed with asset_hash_mismatch.
    // (Implement an SSE consumer here; assert we see the failed event for
    // accepted.install_id with error == "asset_hash_mismatch".)
    let _ = accepted;
    let _ = resp;
    panic!("FIXME: implement SSE consumer using the daemon's existing test scaffolding");
}

async fn start_test_daemon() -> u16 {
    // Use the daemon crate's existing test harness; find it via
    // `grep -rn "fn start_test_daemon\|TestApp\|spawn_test" super-stt-daemon/tests/`
    // and copy the pattern.
    panic!("FIXME: copy from existing daemon test harness in super-stt-daemon/tests/");
}
```

The two `FIXME`s in this test are real handover points — the existing daemon test scaffolding pattern needs to be consulted. Find it via `grep -rn "spawn\|TestApp" super-stt-daemon/tests/` and adapt.

- [ ] **Step 2: Run the test**

```bash
cargo test -p super-stt-daemon --lib    # the existing daemon lib tests still pass
cargo test -p super-stt-daemon --test registry_http -- --ignored    # the new ones
```

- [ ] **Step 3: Commit**

```bash
git add super-stt-daemon/tests/registry_http.rs
git commit -m "test(daemon/registry): HTTP integration tests for /registry/*"
```

---

### Task 17: Document the daemon's new outbound HTTPS surface

**Files:**
- Modify: `docs/SECURITY.md`

- [ ] **Step 1: Read the existing `SECURITY.md`**

Read the current file to understand the section structure.

- [ ] **Step 2: Add a section "Daemon outbound network surface"**

Append (or insert in the appropriate place):

```markdown
## Daemon outbound network surface

Prior to the backend registry (see
`docs/superpowers/specs/2026-05-29-backend-registry-design.md`), the daemon
process made no outbound network connections; only wasm-backend modules
issued HTTPS via `wasi:http`, constrained by per-backend `allowed_hosts`.

After the registry work, the daemon process itself opens HTTPS to:

- `jorge-menjivar.github.io` — registry index (`index.json`).
- `api.github.com` — Custom-repo install flow only (latest-release lookup,
  `backend.toml` fetch).
- `objects.githubusercontent.com` and `github.com` — release asset
  downloads.

No keyring secrets cross these boundaries. Secrets are read at model load
time only, not at install time.

The integrity story for installed assets is SHA-256 verification against the
registry's index, computed by the indexer at index-build time. A compromised
Pages host could serve a malicious index, but cannot bypass the wasm
sandbox or recover keyring secrets through the install path. Custom-repo
installs are explicitly unverified — only TLS protects them — and the daemon
surfaces this to the client.
```

- [ ] **Step 3: Commit**

```bash
git add docs/SECURITY.md
git commit -m "docs(security): daemon outbound HTTPS surface for the registry"
```
