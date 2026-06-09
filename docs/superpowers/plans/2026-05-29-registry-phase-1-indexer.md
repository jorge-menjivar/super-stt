# Registry Phase 1: Indexer + Repo + Workflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `registry/registry.toml` + a GitHub Actions workflow that builds and publishes `index.json` on GitHub Pages. After Phase 1, the registry is live and accepting community PRs — no client work yet.

**Architecture:** A small standalone Rust crate (`registry/scripts/build_index/`) runs in CI on cron + push. It reads `registry/registry.toml`, queries the GitHub API for each entry's latest release (honoring `tag_prefix` + `max_version`), validates the maintainer's `backend.toml`, computes SHA-256 over each asset, carries forward last-known-good on failure, and writes `index.json` to a Pages-published branch. The three existing in-tree backends (`backends/openai`, `mistral`, `voxtral`) become the seed entries — they ship as monorepo entries with `tag_prefix` and migrate to their own repos later with no observable client change.

**Tech Stack:**
- Rust (edition 2024, workspace deps where shared) — the indexer crate is standalone (excluded from workspace, like `backends/*`)
- `reqwest` (workspace), `serde`, `toml`, `ring` for SHA-256, `tar` + `flate2` for tarball inspection, `semver` for tag parsing
- `mockito` for HTTP fixtures in tests
- GitHub Actions YAML (workflow runs ubuntu-latest, uses `actions/checkout@v6`)

---

## File Structure

**Create:**
- `registry/registry.toml` — seed entries (openai, mistral, voxtral)
- `registry/README.md` — submission rules + reserved ids
- `registry/scripts/build_index/Cargo.toml` — standalone crate manifest
- `registry/scripts/build_index/Cargo.lock`
- `registry/scripts/build_index/src/main.rs` — orchestration
- `registry/scripts/build_index/src/registry_toml.rs` — parse + validate the registry file
- `registry/scripts/build_index/src/github.rs` — typed GitHub REST client
- `registry/scripts/build_index/src/resolve.rs` — tag resolution (latest, prefix, max_version)
- `registry/scripts/build_index/src/manifest.rs` — fetch + validate per-backend `backend.toml`
- `registry/scripts/build_index/src/assets.rs` — per-variant validation + SHA-256
- `registry/scripts/build_index/src/license.rs` — allowlist
- `registry/scripts/build_index/src/carryforward.rs` — last-known-good logic
- `registry/scripts/build_index/src/index_json.rs` — output schema
- `registry/scripts/build_index/tests/integration.rs` — end-to-end with mocked GH
- `.github/workflows/build-index.yml` — cron + push trigger

**Modify:**
- `Cargo.toml` (root) — add `"registry/scripts/build_index"` to `exclude`
- `docs/protocol/backend/config.md` — document the `[assets]` matrix
- `backends/openai/backend.toml` — add `[assets.wasm]`
- `backends/mistral/backend.toml` — add `[assets.wasm]`
- `backends/voxtral/backend.toml` — add `[[assets.subprocess]]` matrix

---

### Task 1: Document the `[assets]` shape in the backend protocol doc

**Files:**
- Modify: `docs/protocol/backend/config.md`

- [ ] **Step 1: Read the existing config doc end-to-end**

Read `docs/protocol/backend/config.md` so you understand the house style. Sections use `## ` headers per top-level key, `### ` for subkeys; field tables use `| Field | Type | Required | Notes |`.

- [ ] **Step 2: Add a new top-level `## [assets]` section after `## [options]`**

Insert this section (exactly):

````markdown
## `[assets]`

Declares the binary artifacts a release publishes, so the registry indexer and
the daemon's installer can find them without guessing. The shape depends on the
backend's `kind`.

Wasm backends declare a single file:

```toml
[assets]
wasm = "openai.wasm"   # filename on the release; must be a `wasm32` component
```

Subprocess backends declare one entry per built variant. Selection axes are
`target`, `accel`, and (when `accel = "cuda"`) `cuda_major`, `cuda_sm`, `cudnn`.

```toml
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
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `file` | string | yes | Filename on the GitHub release. Subprocess: `.tar.gz`; wasm: `.wasm`. |
| `target` | string | yes | Rust target triple. Tier-1/2 only; indexer rejects unknown. |
| `accel` | string | yes | One of `"cpu"`, `"cuda"`, `"metal"`, `"rocm"`, `"vulkan"`. |
| `cuda_major` | integer | iff `accel = "cuda"` | CUDA major version this build targets. |
| `cuda_sm` | integer | iff `accel = "cuda"` | Compute capability (e.g. `75`, `86`, `90`). |
| `cudnn` | bool | no | Defaults `false`. Allowed only when `accel = "cuda"`. |

### Subprocess archive contents

A subprocess `.tar.gz` MUST contain `bin/<entrypoint>` (the path that the
backend's `[backend].entrypoint` resolves to after extraction). Tarballs
containing path-traversal entries (`..`, absolute paths) or symlinks that
escape the archive root are rejected by the registry indexer and by the
daemon's installer.
````

- [ ] **Step 3: Commit**

```bash
git add docs/protocol/backend/config.md
git commit -m "docs(protocol): document [assets] matrix in backend.toml"
```

---

### Task 2: Update `backends/openai/backend.toml` with `[assets.wasm]`

**Files:**
- Modify: `backends/openai/backend.toml`

- [ ] **Step 1: Read the current file**

Open `backends/openai/backend.toml`. Note the current `entrypoint = "openai.wasm"` field exists but no `[assets]` section.

- [ ] **Step 2: Add an `[assets]` section after `[network]`**

Insert these two lines (the `entrypoint` field stays — it's the runtime load path; `[assets]` is the registry distribution shape):

```toml
[assets]
    wasm = "openai.wasm"
```

- [ ] **Step 3: Commit**

```bash
git add backends/openai/backend.toml
git commit -m "feat(backends/openai): declare [assets.wasm] for registry"
```

---

### Task 3: Update `backends/mistral/backend.toml` with `[assets.wasm]`

**Files:**
- Modify: `backends/mistral/backend.toml`

- [ ] **Step 1: Open `backends/mistral/backend.toml`**

Find the entrypoint filename used today (search for `entrypoint =`).

- [ ] **Step 2: Add `[assets]` block**

```toml
[assets]
    wasm = "mistral.wasm"   # use the actual entrypoint filename from the file
```

- [ ] **Step 3: Commit**

```bash
git add backends/mistral/backend.toml
git commit -m "feat(backends/mistral): declare [assets.wasm] for registry"
```

---

### Task 4: Update `backends/voxtral/backend.toml` with the subprocess matrix

**Files:**
- Modify: `backends/voxtral/backend.toml`

- [ ] **Step 1: Open `backends/voxtral/backend.toml`**

The current file has `entrypoint = "voxtral"` (the binary path post-extraction). No `[assets]` exists yet.

- [ ] **Step 2: Add the `[[assets.subprocess]]` matrix**

Insert after the existing top-level sections. Match the variants the release CI in `.github/workflows/release.yml` produces today (CPU + CUDA 12 sm 75/80/86/89/90, CUDA 12 + cuDNN for the same sm set, CUDA 13 sm 75/80/86/89/90, CUDA 13 + cuDNN; plus aarch64 CPU). Asset filenames follow the pattern `voxtral-<target>-<suffix>.tar.gz` where `<suffix>` matches the existing CI matrix's `suffix` column (`cpu`, `cuda12-sm75`, `cuda12-cudnn-sm75`, etc.):

```toml
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
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-sm80.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 12
    cuda_sm    = 80

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-sm86.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 12
    cuda_sm    = 86

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-sm89.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 12
    cuda_sm    = 89

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-sm90.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 12
    cuda_sm    = 90

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-cudnn-sm75.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 12
    cuda_sm    = 75
    cudnn      = true

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-cudnn-sm80.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 12
    cuda_sm    = 80
    cudnn      = true

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-cudnn-sm86.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 12
    cuda_sm    = 86
    cudnn      = true

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-cudnn-sm89.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 12
    cuda_sm    = 89
    cudnn      = true

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-cudnn-sm90.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 12
    cuda_sm    = 90
    cudnn      = true

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda13-sm75.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 13
    cuda_sm    = 75

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda13-sm80.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 13
    cuda_sm    = 80

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda13-sm86.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 13
    cuda_sm    = 86

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda13-sm89.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 13
    cuda_sm    = 89

[[assets.subprocess]]
    file       = "voxtral-x86_64-unknown-linux-gnu-cuda13-sm90.tar.gz"
    target     = "x86_64-unknown-linux-gnu"
    accel      = "cuda"
    cuda_major = 13
    cuda_sm    = 90

[[assets.subprocess]]
    file   = "voxtral-aarch64-unknown-linux-gnu-cpu.tar.gz"
    target = "aarch64-unknown-linux-gnu"
    accel  = "cpu"
```

Before committing, **check `.github/workflows/release.yml` line ~75 onward** to confirm the asset filename pattern and matrix entries. Add CUDA 13 cuDNN variants if the release workflow ships them; remove any variants the current workflow doesn't produce.

- [ ] **Step 3: Commit**

```bash
git add backends/voxtral/backend.toml
git commit -m "feat(backends/voxtral): declare [[assets.subprocess]] matrix"
```

---

### Task 5: Bootstrap the indexer crate skeleton

**Files:**
- Create: `registry/scripts/build_index/Cargo.toml`
- Create: `registry/scripts/build_index/src/main.rs`
- Modify: `Cargo.toml` (root)

- [ ] **Step 1: Create the crate directory and `Cargo.toml`**

```bash
mkdir -p registry/scripts/build_index/src
```

Write `registry/scripts/build_index/Cargo.toml`:

```toml
# SPDX-License-Identifier: GPL-3.0-only
[package]
name = "super-stt-build-index"
version = "0.1.0"
edition = "2024"
license = "GPL-3.0-only"
description = "Cron-driven indexer that builds index.json from registry.toml"

[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
flate2 = "1"
hex = "0.4"
log = "0.4"
env_logger = "0.11"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "stream"] }
ring = "0.17"
semver = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tar = "0.4"
thiserror = "2"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "fs", "io-util"] }
toml = "0.8"
url = "2"

[dev-dependencies]
mockito = "1"
tempfile = "3"
```

- [ ] **Step 2: Write a stub `main.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! `super-stt-build-index` — read `registry.toml`, fetch GitHub release
//! metadata for every entry, validate, and emit `index.json`.
//!
//! Run from CI; not in the workspace.

fn main() {
    println!("super-stt-build-index v0.1.0");
}
```

- [ ] **Step 3: Add the crate to the workspace `exclude` list**

Open `Cargo.toml` at repo root. Find the `exclude = [...]` array under `[workspace]`. Add `"registry/scripts/build_index"` in alphabetical order.

- [ ] **Step 4: Verify it builds standalone**

Run: `cd registry/scripts/build_index && cargo build`
Expected: compiles successfully, produces `target/debug/super-stt-build-index`.

- [ ] **Step 5: Verify the workspace build still passes**

Run from repo root: `cargo check --workspace`
Expected: no errors; the new crate is not built (because it's excluded).

- [ ] **Step 6: Commit**

```bash
git add registry/scripts/build_index/ Cargo.toml
git commit -m "feat(registry): bootstrap build_index crate skeleton"
```

---

### Task 6: Parse + validate `registry.toml`

**Files:**
- Create: `registry/scripts/build_index/src/registry_toml.rs`
- Modify: `registry/scripts/build_index/src/main.rs`
- Test: `registry/scripts/build_index/src/registry_toml.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write `registry_toml.rs` with the data model**

Create `registry/scripts/build_index/src/registry_toml.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Parse and structurally validate `registry/registry.toml`.

use std::collections::BTreeMap;

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub repo: String,
    #[serde(default)]
    pub subdir: Option<String>,
    #[serde(default)]
    pub tag_prefix: Option<String>,
    #[serde(default)]
    pub max_version: Option<String>,
    #[serde(default)]
    pub removed: bool,
    #[serde(default)]
    pub removed_reason: Option<String>,
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("entry `{id}`: {reason}")]
    Entry { id: String, reason: String },
    #[error("entries `{a}` and `{b}` share repo `{repo}` but at least one has no `tag_prefix`")]
    MonorepoMissingPrefix { a: String, b: String, repo: String },
    #[error("entries `{a}` and `{b}` share repo `{repo}` and the same `tag_prefix = {prefix:?}`")]
    PrefixCollision { a: String, b: String, repo: String, prefix: String },
}

/// Parsed registry file: id → entry, preserving the file's order.
#[derive(Debug, Clone)]
pub struct Registry(pub BTreeMap<String, Entry>);

impl Registry {
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        let raw: BTreeMap<String, Entry> = toml::from_str(input)?;
        for (id, e) in &raw {
            validate_entry(id, e)?;
        }
        validate_monorepo_groups(&raw)?;
        Ok(Self(raw))
    }
}

fn validate_entry(id: &str, e: &Entry) -> Result<(), ParseError> {
    if !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
        return Err(ParseError::Entry {
            id: id.into(),
            reason: "id must be ascii lowercase, digits, `-`, `_`".into(),
        });
    }
    if e.repo.is_empty() {
        return Err(ParseError::Entry { id: id.into(), reason: "`repo` is required".into() });
    }
    if let Some(sd) = &e.subdir {
        if sd.contains("..") || sd.starts_with('/') || sd.contains('\\') {
            return Err(ParseError::Entry {
                id: id.into(),
                reason: format!("`subdir = {sd:?}` is not a safe relative path"),
            });
        }
    }
    Ok(())
}

fn validate_monorepo_groups(raw: &BTreeMap<String, Entry>) -> Result<(), ParseError> {
    let mut by_repo: BTreeMap<&str, Vec<(&str, Option<&str>)>> = BTreeMap::new();
    for (id, e) in raw {
        if e.removed { continue; }
        by_repo.entry(e.repo.as_str()).or_default().push((id, e.tag_prefix.as_deref()));
    }
    for (repo, members) in &by_repo {
        if members.len() < 2 { continue; }
        for (i, (a, prefix_a)) in members.iter().enumerate() {
            if prefix_a.is_none() {
                let (b, _) = members.iter().find(|(b, _)| b != a).unwrap();
                return Err(ParseError::MonorepoMissingPrefix {
                    a: (*a).into(), b: (*b).into(), repo: (*repo).into(),
                });
            }
            for (b, prefix_b) in &members[i+1..] {
                if prefix_a == prefix_b {
                    return Err(ParseError::PrefixCollision {
                        a: (*a).into(), b: (*b).into(),
                        repo: (*repo).into(),
                        prefix: prefix_a.unwrap_or_default().into(),
                    });
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_entry() {
        let r = Registry::parse(r#"
            [openai]
            repo = "github.com/jorge-menjivar/super-stt"
            subdir = "backends/openai"
            tag_prefix = "openai-"
        "#).unwrap();
        let e = r.0.get("openai").unwrap();
        assert_eq!(e.repo, "github.com/jorge-menjivar/super-stt");
        assert_eq!(e.subdir.as_deref(), Some("backends/openai"));
        assert_eq!(e.tag_prefix.as_deref(), Some("openai-"));
    }

    #[test]
    fn rejects_path_traversal_in_subdir() {
        let err = Registry::parse(r#"
            [bad]
            repo = "github.com/x/y"
            subdir = "../escape"
        "#).unwrap_err();
        assert!(matches!(err, ParseError::Entry { .. }));
    }

    #[test]
    fn rejects_monorepo_without_tag_prefix() {
        let err = Registry::parse(r#"
            [a]
            repo = "github.com/x/mono"
            [b]
            repo = "github.com/x/mono"
        "#).unwrap_err();
        assert!(matches!(err, ParseError::MonorepoMissingPrefix { .. }));
    }

    #[test]
    fn rejects_tag_prefix_collision() {
        let err = Registry::parse(r#"
            [a]
            repo = "github.com/x/mono"
            tag_prefix = "v"
            [b]
            repo = "github.com/x/mono"
            tag_prefix = "v"
        "#).unwrap_err();
        assert!(matches!(err, ParseError::PrefixCollision { .. }));
    }

    #[test]
    fn allows_two_distinct_prefixes_on_same_repo() {
        let r = Registry::parse(r#"
            [a]
            repo = "github.com/x/mono"
            tag_prefix = "a-"
            [b]
            repo = "github.com/x/mono"
            tag_prefix = "b-"
        "#).unwrap();
        assert_eq!(r.0.len(), 2);
    }

    #[test]
    fn removed_entries_dont_count_for_collision() {
        // Two entries on the same repo, one removed → no collision.
        let r = Registry::parse(r#"
            [a]
            repo = "github.com/x/mono"
            [b]
            repo = "github.com/x/mono"
            removed = true
        "#).unwrap();
        assert_eq!(r.0.len(), 2);
    }
}
```

- [ ] **Step 2: Add the module to `main.rs`**

Update `registry/scripts/build_index/src/main.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-only
mod registry_toml;

fn main() {
    println!("super-stt-build-index v0.1.0");
}
```

- [ ] **Step 3: Run the tests**

Run: `cd registry/scripts/build_index && cargo test`
Expected: all 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add registry/scripts/build_index/
git commit -m "feat(registry/build_index): parse + validate registry.toml"
```

---

### Task 7: GitHub REST client wrapper

**Files:**
- Create: `registry/scripts/build_index/src/github.rs`
- Modify: `registry/scripts/build_index/src/main.rs`
- Test: inline `#[cfg(test)]` using `mockito`

- [ ] **Step 1: Write the client**

Create `registry/scripts/build_index/src/github.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Minimal GitHub REST client — only the endpoints the indexer needs.

use serde::Deserialize;

#[derive(Clone)]
pub struct GitHub {
    base: String,
    http: reqwest::Client,
    token: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Debug, Deserialize)]
struct ContentResponse {
    content: String,    // base64-encoded
    encoding: String,
}

impl GitHub {
    pub fn new(base: impl Into<String>, token: Option<String>) -> Self {
        Self { base: base.into(), http: reqwest::Client::new(), token }
    }

    pub fn from_env() -> Self {
        Self::new("https://api.github.com", std::env::var("GITHUB_TOKEN").ok())
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut b = self.http.request(method, format!("{}{path}", self.base))
            .header("User-Agent", "super-stt-build-index/0.1")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(t) = &self.token {
            b = b.bearer_auth(t);
        }
        b
    }

    pub async fn latest_release(&self, owner_repo: &str) -> anyhow::Result<Release> {
        let r = self.req(reqwest::Method::GET, &format!("/repos/{owner_repo}/releases/latest"))
            .send().await?.error_for_status()?;
        Ok(r.json().await?)
    }

    pub async fn list_releases(&self, owner_repo: &str) -> anyhow::Result<Vec<Release>> {
        // GitHub paginates; 100 is the max page size. For the indexer use case
        // a single page is enough — backends with >100 releases are out of scope.
        let r = self.req(reqwest::Method::GET, &format!("/repos/{owner_repo}/releases?per_page=100"))
            .send().await?.error_for_status()?;
        Ok(r.json().await?)
    }

    pub async fn fetch_file(&self, owner_repo: &str, path: &str, r#ref: &str) -> anyhow::Result<Vec<u8>> {
        // GET /repos/{owner_repo}/contents/{path}?ref={ref}
        let r = self.req(reqwest::Method::GET,
            &format!("/repos/{owner_repo}/contents/{path}?ref={ref}"))
            .send().await?.error_for_status()?;
        let body: ContentResponse = r.json().await?;
        anyhow::ensure!(body.encoding == "base64", "unexpected encoding `{}`", body.encoding);
        use base64::Engine;
        Ok(base64::engine::general_purpose::STANDARD.decode(body.content.replace('\n', ""))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;

    #[tokio::test]
    async fn latest_release_returns_tag() {
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/repos/x/y/releases/latest")
            .with_status(200).with_body(r#"{"tag_name":"v1.2.3","assets":[]}"#)
            .create_async().await;
        let gh = GitHub::new(s.url(), None);
        let r = gh.latest_release("x/y").await.unwrap();
        assert_eq!(r.tag_name, "v1.2.3");
    }

    #[tokio::test]
    async fn fetch_file_decodes_base64() {
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", Matcher::Regex(r"^/repos/x/y/contents/backend\.toml.*".into()))
            .with_status(200)
            .with_body(r#"{"content":"aGVsbG8=","encoding":"base64"}"#)
            .create_async().await;
        let gh = GitHub::new(s.url(), None);
        let bytes = gh.fetch_file("x/y", "backend.toml", "v1.2.3").await.unwrap();
        assert_eq!(&bytes, b"hello");
    }
}
```

- [ ] **Step 2: Add `base64` to `Cargo.toml`**

```toml
base64 = "0.22"
```

- [ ] **Step 3: Register the module in `main.rs`**

```rust
mod github;
mod registry_toml;
```

- [ ] **Step 4: Run tests**

Run: `cd registry/scripts/build_index && cargo test`
Expected: all tests (including new GitHub client tests) pass.

- [ ] **Step 5: Commit**

```bash
git add registry/scripts/build_index/
git commit -m "feat(registry/build_index): GitHub REST client"
```

---

### Task 8: Tag resolution

**Files:**
- Create: `registry/scripts/build_index/src/resolve.rs`
- Modify: `registry/scripts/build_index/src/main.rs`

- [ ] **Step 1: Write `resolve.rs`**

Create `registry/scripts/build_index/src/resolve.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Resolve the version + tag the indexer should publish for an entry.

use semver::Version;
use thiserror::Error;

use crate::github::{GitHub, Release};
use crate::registry_toml::Entry;

#[derive(Debug, Clone)]
pub struct Resolved {
    pub tag: String,
    pub version: Version,
    pub release: Release,
}

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("no releases on `{repo}`")]
    NoReleases { repo: String },
    #[error("no release tag matches prefix `{prefix}`")]
    NoMatchingPrefix { prefix: String },
    #[error("tag `{tag}` does not parse as semver after stripping prefix `{prefix:?}`")]
    BadSemver { tag: String, prefix: Option<String> },
    #[error(transparent)]
    Http(#[from] anyhow::Error),
}

pub async fn resolve(gh: &GitHub, owner_repo: &str, entry: &Entry) -> Result<Resolved, ResolveError> {
    let releases = if entry.tag_prefix.is_some() {
        gh.list_releases(owner_repo).await?
    } else {
        vec![gh.latest_release(owner_repo).await?]
    };
    select_release(releases, entry)
}

fn select_release(releases: Vec<Release>, entry: &Entry) -> Result<Resolved, ResolveError> {
    let max_cap = entry.max_version.as_deref().map(parse_semver).transpose()
        .map_err(|tag| ResolveError::BadSemver { tag, prefix: None })?;

    let mut best: Option<(Version, Release)> = None;
    for r in releases {
        if r.draft || r.prerelease { continue; }
        let stripped = match &entry.tag_prefix {
            Some(p) => match r.tag_name.strip_prefix(p.as_str()) { Some(rest) => rest, None => continue },
            None => r.tag_name.strip_prefix('v').unwrap_or(&r.tag_name),
        };
        let v = match Version::parse(stripped) {
            Ok(v) => v,
            Err(_) => continue,    // unparseable tags are ignored, not errors
        };
        if let Some(cap) = &max_cap {
            if &v > cap { continue; }
        }
        match &best {
            Some((cur, _)) if cur >= &v => {}
            _ => best = Some((v, r)),
        }
    }
    let (version, release) = best.ok_or_else(|| match &entry.tag_prefix {
        Some(p) => ResolveError::NoMatchingPrefix { prefix: p.clone() },
        None => ResolveError::NoReleases { repo: entry.repo.clone() },
    })?;
    Ok(Resolved { tag: release.tag_name.clone(), version, release })
}

fn parse_semver(s: &str) -> Result<Version, String> {
    Version::parse(s.strip_prefix('v').unwrap_or(s)).map_err(|_| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry_toml::Entry;
    use crate::github::Release;

    fn rel(tag: &str) -> Release {
        Release { tag_name: tag.into(), draft: false, prerelease: false, assets: vec![] }
    }

    fn entry(prefix: Option<&str>, max: Option<&str>) -> Entry {
        Entry {
            repo: "github.com/x/y".into(), subdir: None,
            tag_prefix: prefix.map(String::from),
            max_version: max.map(String::from),
            removed: false, removed_reason: None,
        }
    }

    #[test]
    fn picks_latest_semver_with_v_prefix() {
        let r = select_release(vec![rel("v1.0.0"), rel("v1.2.3"), rel("v1.1.0")], &entry(None, None)).unwrap();
        assert_eq!(r.tag, "v1.2.3");
        assert_eq!(r.version, Version::new(1, 2, 3));
    }

    #[test]
    fn honors_max_version_cap() {
        let r = select_release(vec![rel("v1.0.0"), rel("v2.0.0")], &entry(None, Some("1.0.0"))).unwrap();
        assert_eq!(r.version, Version::new(1, 0, 0));
    }

    #[test]
    fn filters_by_tag_prefix() {
        let r = select_release(vec![rel("openai-1.0.0"), rel("voxtral-2.0.0"), rel("openai-1.5.0")],
            &entry(Some("openai-"), None)).unwrap();
        assert_eq!(r.version, Version::new(1, 5, 0));
    }

    #[test]
    fn skips_prerelease() {
        let mut releases = vec![rel("v1.0.0"), rel("v2.0.0")];
        releases[1].prerelease = true;
        let r = select_release(releases, &entry(None, None)).unwrap();
        assert_eq!(r.version, Version::new(1, 0, 0));
    }

    #[test]
    fn errors_when_no_match() {
        let err = select_release(vec![rel("v1.0.0")], &entry(Some("xyz-"), None)).unwrap_err();
        assert!(matches!(err, ResolveError::NoMatchingPrefix { .. }));
    }

    #[test]
    fn ignores_unparseable_tags() {
        let r = select_release(vec![rel("not-semver"), rel("v0.1.0")], &entry(None, None)).unwrap();
        assert_eq!(r.version, Version::new(0, 1, 0));
    }
}
```

- [ ] **Step 2: Register the module**

Add `mod resolve;` to `main.rs`.

- [ ] **Step 3: Run tests**

Run: `cd registry/scripts/build_index && cargo test`
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add registry/scripts/build_index/
git commit -m "feat(registry/build_index): tag resolution with prefix + max_version"
```

---

### Task 9: Fetch + validate per-backend `backend.toml`

**Files:**
- Create: `registry/scripts/build_index/src/manifest.rs`
- Modify: `registry/scripts/build_index/src/main.rs`

- [ ] **Step 1: Write `manifest.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Fetch + structurally validate a backend's `backend.toml` at a tag.

use std::collections::BTreeMap;

use semver::Version;
use serde::Deserialize;
use thiserror::Error;

use crate::github::GitHub;

pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub backend: BackendMeta,
    #[serde(default)]
    pub network: Network,
    #[serde(default)]
    pub models: Vec<Model>,
    #[serde(default)]
    pub secrets: Vec<Secret>,
    #[serde(default)]
    pub options: Vec<Option_>,
    pub assets: Assets,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackendMeta {
    pub source: String,
    pub name: String,
    pub version: String,
    pub kind: String,            // "wasm" | "subprocess"
    pub entrypoint: String,
    pub contract: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Network {
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Model {
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub supported_devices: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Secret {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename = "Option")]
pub struct Option_ {
    pub name: String,
    pub label: String,
    pub r#type: String,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Assets {
    #[serde(default)]
    pub wasm: Option<String>,
    #[serde(default)]
    pub subprocess: Vec<SubprocessAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubprocessAsset {
    pub file: String,
    pub target: String,
    pub accel: String,
    #[serde(default)]
    pub cuda_major: Option<u32>,
    #[serde(default)]
    pub cuda_sm: Option<u32>,
    #[serde(default)]
    pub cudnn: bool,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("manifest exceeds {MAX_MANIFEST_BYTES} bytes")]
    TooLarge,
    #[error("TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("`backend.version = {0:?}` does not match tag version `{1}`")]
    VersionMismatch(String, Version),
    #[error("`backend.source = {0:?}` does not match registry entry repo `{1}`")]
    SourceMismatch(String, String),
    #[error("`backend.kind = {0:?}` requires `[assets.wasm]` but it is missing")]
    MissingWasmAsset(String),
    #[error("`backend.kind = {0:?}` requires `[[assets.subprocess]]` but list is empty")]
    MissingSubprocessAssets(String),
    #[error("backend `kind` must be `wasm` or `subprocess` (got {0:?})")]
    UnknownKind(String),
    #[error("subprocess asset `{file}`: `accel = {accel:?}` is not allowed")]
    UnknownAccel { file: String, accel: String },
    #[error("subprocess asset `{file}`: cuda_major/cuda_sm required when accel = \"cuda\"")]
    CudaMissingFields { file: String },
    #[error("subprocess asset `{file}`: cuda_major/cuda_sm forbidden when accel != \"cuda\"")]
    CudaForbiddenFields { file: String },
    #[error("subprocess asset `{file}`: `cudnn = true` requires `accel = \"cuda\"`")]
    CudnnRequiresCuda { file: String },
    #[error("missing license; declare `[backend].license`")]
    MissingLicense,
    #[error("license `{0}` is not on the allowlist")]
    LicenseNotAllowed(String),
    #[error(transparent)]
    Http(#[from] anyhow::Error),
}

const ALLOWED_ACCEL: &[&str] = &["cpu", "cuda", "metal", "rocm", "vulkan"];

pub async fn fetch(gh: &GitHub, owner_repo: &str, subdir: Option<&str>, tag: &str) -> Result<Manifest, ManifestError> {
    let path = match subdir {
        Some(sd) => format!("{}/backend.toml", sd.trim_end_matches('/')),
        None => "backend.toml".to_string(),
    };
    let bytes = gh.fetch_file(owner_repo, &path, tag).await?;
    if bytes.len() > MAX_MANIFEST_BYTES { return Err(ManifestError::TooLarge); }
    let text = String::from_utf8(bytes).map_err(|e| anyhow::anyhow!("manifest not UTF-8: {e}"))?;
    let m: Manifest = toml::from_str(&text)?;
    Ok(m)
}

pub fn validate(m: &Manifest, expected_version: &Version, expected_source: &str) -> Result<(), ManifestError> {
    let v = Version::parse(m.backend.version.trim_start_matches('v'))
        .map_err(|_| ManifestError::VersionMismatch(m.backend.version.clone(), expected_version.clone()))?;
    if &v != expected_version {
        return Err(ManifestError::VersionMismatch(m.backend.version.clone(), expected_version.clone()));
    }
    if m.backend.source != expected_source {
        return Err(ManifestError::SourceMismatch(m.backend.source.clone(), expected_source.into()));
    }
    match m.backend.kind.as_str() {
        "wasm" => { if m.assets.wasm.is_none() { return Err(ManifestError::MissingWasmAsset(m.backend.kind.clone())); } }
        "subprocess" => { if m.assets.subprocess.is_empty() { return Err(ManifestError::MissingSubprocessAssets(m.backend.kind.clone())); } }
        other => return Err(ManifestError::UnknownKind(other.into())),
    }
    for a in &m.assets.subprocess {
        if !ALLOWED_ACCEL.contains(&a.accel.as_str()) {
            return Err(ManifestError::UnknownAccel { file: a.file.clone(), accel: a.accel.clone() });
        }
        if a.accel == "cuda" {
            if a.cuda_major.is_none() || a.cuda_sm.is_none() {
                return Err(ManifestError::CudaMissingFields { file: a.file.clone() });
            }
        } else {
            if a.cuda_major.is_some() || a.cuda_sm.is_some() {
                return Err(ManifestError::CudaForbiddenFields { file: a.file.clone() });
            }
            if a.cudnn {
                return Err(ManifestError::CudnnRequiresCuda { file: a.file.clone() });
            }
        }
    }
    crate::license::check(m.backend.license.as_deref())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
        [backend]
        source = "github.com/x/y"
        name = "Y"
        version = "1.0.0"
        kind = "wasm"
        entrypoint = "y.wasm"
        contract = "v1"
        license = "Apache-2.0"

        [assets]
        wasm = "y.wasm"
    "#;

    #[test]
    fn validates_a_correct_wasm_manifest() {
        let m: Manifest = toml::from_str(VALID).unwrap();
        validate(&m, &Version::new(1, 0, 0), "github.com/x/y").unwrap();
    }

    #[test]
    fn rejects_version_mismatch() {
        let m: Manifest = toml::from_str(VALID).unwrap();
        let err = validate(&m, &Version::new(2, 0, 0), "github.com/x/y").unwrap_err();
        assert!(matches!(err, ManifestError::VersionMismatch(_, _)));
    }

    #[test]
    fn rejects_source_mismatch() {
        let m: Manifest = toml::from_str(VALID).unwrap();
        let err = validate(&m, &Version::new(1, 0, 0), "github.com/other/repo").unwrap_err();
        assert!(matches!(err, ManifestError::SourceMismatch(_, _)));
    }

    #[test]
    fn rejects_cuda_without_required_fields() {
        let t = r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"
            license = "Apache-2.0"

            [[assets.subprocess]]
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = "cuda"
        "#;
        let m: Manifest = toml::from_str(t).unwrap();
        let err = validate(&m, &Version::new(1, 0, 0), "github.com/x/y").unwrap_err();
        assert!(matches!(err, ManifestError::CudaMissingFields { .. }));
    }
}
```

- [ ] **Step 2: Create `license.rs` (referenced above)**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! License allowlist.

use crate::manifest::ManifestError;

const ALLOWED: &[&str] = &[
    "Apache-2.0", "MIT", "BSD-2-Clause", "BSD-3-Clause", "MPL-2.0",
    "GPL-3.0-only", "GPL-3.0-or-later", "ISC",
];

pub fn check(license: Option<&str>) -> Result<(), ManifestError> {
    let lic = license.ok_or(ManifestError::MissingLicense)?;
    if !ALLOWED.contains(&lic) {
        return Err(ManifestError::LicenseNotAllowed(lic.into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_apache() { check(Some("Apache-2.0")).unwrap(); }

    #[test]
    fn rejects_unknown() {
        let err = check(Some("Proprietary")).unwrap_err();
        assert!(matches!(err, ManifestError::LicenseNotAllowed(_)));
    }

    #[test]
    fn rejects_missing() {
        let err = check(None).unwrap_err();
        assert!(matches!(err, ManifestError::MissingLicense));
    }
}
```

- [ ] **Step 3: Register both modules**

Add to `main.rs`:
```rust
mod license;
mod manifest;
```

- [ ] **Step 4: Run tests**

Run: `cd registry/scripts/build_index && cargo test`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add registry/scripts/build_index/
git commit -m "feat(registry/build_index): manifest fetch + validation + license allowlist"
```

---

### Task 10: Asset validation (size, wasm magic, tar safety) + SHA-256 streaming

**Files:**
- Create: `registry/scripts/build_index/src/assets.rs`
- Modify: `registry/scripts/build_index/src/main.rs`

- [ ] **Step 1: Write `assets.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Per-asset validation + SHA-256 over streamed downloads.

use std::io::Read;

use ring::digest::{Context, SHA256};
use thiserror::Error;

use crate::manifest::{Assets, SubprocessAsset};

pub const MAX_ASSET_BYTES: u64 = 200 * 1024 * 1024;
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6d];

#[derive(Debug, Error)]
pub enum AssetError {
    #[error("asset `{0}` is missing from the release")]
    Missing(String),
    #[error("asset `{file}` size {size} exceeds {MAX_ASSET_BYTES}")]
    TooLarge { file: String, size: u64 },
    #[error("asset `{0}` does not start with the wasm32 magic header")]
    NotWasm(String),
    #[error("tarball `{file}` contains escape entry `{entry}`")]
    TarEscape { file: String, entry: String },
    #[error("tarball `{file}` does not contain `bin/{entrypoint}`")]
    TarMissingEntrypoint { file: String, entrypoint: String },
    #[error(transparent)]
    Http(#[from] anyhow::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Resolve a declared asset's URL via the release manifest, refusing if missing.
pub fn resolve_url(
    file: &str,
    release_assets: &[crate::github::ReleaseAsset],
) -> Result<(String, u64), AssetError> {
    let a = release_assets.iter().find(|a| a.name == file)
        .ok_or_else(|| AssetError::Missing(file.into()))?;
    if a.size > MAX_ASSET_BYTES {
        return Err(AssetError::TooLarge { file: file.into(), size: a.size });
    }
    Ok((a.browser_download_url.clone(), a.size))
}

/// Stream the asset, compute SHA-256, and dispatch validation based on kind.
pub async fn fetch_and_validate(
    http: &reqwest::Client,
    url: &str,
    expected: AssetExpect<'_>,
) -> Result<String, AssetError> {
    use futures::StreamExt;
    let mut resp = http.get(url).send().await.map_err(|e| AssetError::Http(e.into()))?
        .error_for_status().map_err(|e| AssetError::Http(e.into()))?
        .bytes_stream();
    let mut ctx = Context::new(&SHA256);
    let mut buf: Vec<u8> = Vec::new();
    let mut first_chunk: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.next().await {
        let chunk = chunk.map_err(|e| AssetError::Http(e.into()))?;
        ctx.update(&chunk);
        if first_chunk.len() < 4 {
            for b in chunk.iter() {
                if first_chunk.len() < 4 { first_chunk.push(*b); }
            }
        }
        if matches!(expected, AssetExpect::Subprocess { .. }) {
            buf.extend_from_slice(&chunk);
            if buf.len() as u64 > MAX_ASSET_BYTES {
                return Err(AssetError::TooLarge { file: expected.file().into(), size: buf.len() as u64 });
            }
        }
    }
    match expected {
        AssetExpect::Wasm { file } => {
            if first_chunk != WASM_MAGIC { return Err(AssetError::NotWasm(file.into())); }
        }
        AssetExpect::Subprocess { file, entrypoint } => {
            validate_tarball(file, entrypoint, &buf)?;
        }
    }
    Ok(hex::encode(ctx.finish().as_ref()))
}

pub enum AssetExpect<'a> {
    Wasm { file: &'a str },
    Subprocess { file: &'a str, entrypoint: &'a str },
}

impl<'a> AssetExpect<'a> {
    fn file(&self) -> &str {
        match self { AssetExpect::Wasm { file } | AssetExpect::Subprocess { file, .. } => file }
    }
}

fn validate_tarball(file: &str, entrypoint: &str, bytes: &[u8]) -> Result<(), AssetError> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    let mut found_entrypoint = false;
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?;
        let s = path.to_string_lossy();
        if s.starts_with('/') || s.contains("..") {
            return Err(AssetError::TarEscape { file: file.into(), entry: s.into() });
        }
        if entry.header().entry_type().is_symlink() {
            // Reject symlinks outright; the daemon's installer also rejects.
            return Err(AssetError::TarEscape { file: file.into(), entry: s.into() });
        }
        if s == format!("bin/{entrypoint}") { found_entrypoint = true; }
    }
    if !found_entrypoint {
        return Err(AssetError::TarMissingEntrypoint { file: file.into(), entrypoint: entrypoint.into() });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn make_tarball<F: FnOnce(&mut tar::Builder<GzEncoder<Vec<u8>>>)>(f: F) -> Vec<u8> {
        let gz = GzEncoder::new(Vec::new(), Compression::default());
        let mut tb = tar::Builder::new(gz);
        f(&mut tb);
        tb.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn accepts_tarball_with_bin_entrypoint() {
        let bytes = make_tarball(|tb| {
            let mut h = tar::Header::new_gnu();
            h.set_size(3); h.set_mode(0o755); h.set_cksum();
            tb.append_data(&mut h, "bin/voxtral", &b"abc"[..]).unwrap();
        });
        validate_tarball("v.tar.gz", "voxtral", &bytes).unwrap();
    }

    #[test]
    fn rejects_tarball_without_entrypoint() {
        let bytes = make_tarball(|tb| {
            let mut h = tar::Header::new_gnu();
            h.set_size(3); h.set_mode(0o644); h.set_cksum();
            tb.append_data(&mut h, "README", &b"abc"[..]).unwrap();
        });
        let err = validate_tarball("v.tar.gz", "voxtral", &bytes).unwrap_err();
        assert!(matches!(err, AssetError::TarMissingEntrypoint { .. }));
    }

    #[test]
    fn rejects_path_traversal() {
        let bytes = make_tarball(|tb| {
            let mut h = tar::Header::new_gnu();
            h.set_size(0); h.set_mode(0o644); h.set_cksum();
            tb.append_data(&mut h, "../escape", &b""[..]).unwrap();
        });
        let err = validate_tarball("v.tar.gz", "voxtral", &bytes).unwrap_err();
        assert!(matches!(err, AssetError::TarEscape { .. }));
    }
}
```

Add `futures = "0.3"` to `Cargo.toml`.

- [ ] **Step 2: Register the module**

Add `mod assets;` to `main.rs`.

- [ ] **Step 3: Run tests**

Run: `cd registry/scripts/build_index && cargo test`
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add registry/scripts/build_index/
git commit -m "feat(registry/build_index): asset validation + SHA-256 streaming"
```

---

### Task 11: Index output schema

**Files:**
- Create: `registry/scripts/build_index/src/index_json.rs`
- Modify: `registry/scripts/build_index/src/main.rs`

- [ ] **Step 1: Write the schema types**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! `index.json` output schema. Mirrors the spec at
//! `docs/superpowers/specs/2026-05-29-backend-registry-design.md`.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
pub const MIN_CLIENT: &str = "0.6.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub schema_version: u32,
    pub generated_at: String,
    pub min_client: String,
    pub backends: Vec<IndexBackend>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexBackend {
    pub id: String,
    pub source: String,
    pub version: String,
    pub tag: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_stale: Option<IndexStale>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexModel {
    pub name: String,
    pub provider: String,
    pub supported_devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSecret {
    pub name: String,
    pub label: String,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexOption {
    pub name: String,
    pub label: String,
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IndexAssets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm: Option<IndexAsset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subprocess: Vec<IndexSubprocessAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexAsset {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSubprocessAsset {
    pub target: String,
    pub accel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_major: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_sm: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cudnn: bool,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStale {
    pub latest_attempted: String,
    pub tag: String,
    pub error: String,
    pub since: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_a_minimal_index() {
        let idx = Index {
            schema_version: SCHEMA_VERSION,
            generated_at: "2026-05-29T18:00:00Z".into(),
            min_client: MIN_CLIENT.into(),
            backends: vec![IndexBackend {
                id: "openai".into(), source: "github.com/x/y".into(),
                version: "1.0.0".into(), tag: "v1.0.0".into(),
                name: "OpenAI".into(), description: None,
                license: "Apache-2.0".into(), kind: "wasm".into(),
                contract: "v1".into(), entrypoint: "openai.wasm".into(),
                allowed_hosts: vec!["api.openai.com".into()],
                online: true, supports_gpu: false, supports_cpu: false,
                models: vec![], secrets: vec![], options: vec![],
                assets: IndexAssets {
                    wasm: Some(IndexAsset {
                        url: "https://x".into(), size: 1, sha256: "abc".into(),
                    }),
                    subprocess: vec![],
                },
                index_stale: None,
            }],
        };
        let s = serde_json::to_string_pretty(&idx).unwrap();
        let back: Index = serde_json::from_str(&s).unwrap();
        assert_eq!(back.backends.len(), 1);
        assert_eq!(back.backends[0].id, "openai");
    }
}
```

- [ ] **Step 2: Register the module + add to `main.rs`**

```rust
mod index_json;
```

- [ ] **Step 3: Run tests + commit**

Run: `cargo test`
Expected: pass.

```bash
git add registry/scripts/build_index/
git commit -m "feat(registry/build_index): index.json schema"
```

---

### Task 12: Carry-forward last-known-good on failure

**Files:**
- Create: `registry/scripts/build_index/src/carryforward.rs`
- Modify: `registry/scripts/build_index/src/main.rs`

- [ ] **Step 1: Write the carry-forward logic**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Last-known-good carry-forward: when a new index build fails for an entry,
//! copy the prior `index.json` entry forward with an added `index_stale` field.

use crate::index_json::{IndexBackend, IndexStale};

pub fn maybe_carry_forward(
    id: &str,
    prior: Option<&IndexBackend>,
    error: &str,
    attempted_version: &str,
    attempted_tag: &str,
    now_iso: &str,
) -> Option<IndexBackend> {
    let prior = prior?;
    if prior.id != id { return None; }
    let mut copy = prior.clone();
    copy.index_stale = Some(IndexStale {
        latest_attempted: attempted_version.into(),
        tag: attempted_tag.into(),
        error: error.into(),
        since: now_iso.into(),
    });
    Some(copy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_json::*;

    fn dummy(id: &str) -> IndexBackend {
        IndexBackend {
            id: id.into(), source: "x".into(), version: "1.0.0".into(),
            tag: "v1.0.0".into(), name: id.into(), description: None,
            license: "Apache-2.0".into(), kind: "wasm".into(),
            contract: "v1".into(), entrypoint: format!("{id}.wasm"),
            allowed_hosts: vec![], online: false, supports_gpu: false, supports_cpu: false,
            models: vec![], secrets: vec![], options: vec![],
            assets: IndexAssets::default(),
            index_stale: None,
        }
    }

    #[test]
    fn carries_forward_with_index_stale_marker() {
        let prior = dummy("openai");
        let carried = maybe_carry_forward("openai", Some(&prior),
            "asset missing", "1.5.0", "v1.5.0", "2026-05-29T18:00:00Z").unwrap();
        assert_eq!(carried.version, "1.0.0");
        let stale = carried.index_stale.unwrap();
        assert_eq!(stale.latest_attempted, "1.5.0");
        assert_eq!(stale.error, "asset missing");
    }

    #[test]
    fn returns_none_when_no_prior() {
        assert!(maybe_carry_forward("openai", None, "x", "1.0.0", "v1.0.0", "now").is_none());
    }
}
```

- [ ] **Step 2: Register the module**

Add `mod carryforward;`.

- [ ] **Step 3: Run tests + commit**

```bash
git add registry/scripts/build_index/
git commit -m "feat(registry/build_index): last-known-good carry-forward"
```

---

### Task 13: `main.rs` orchestration

**Files:**
- Modify: `registry/scripts/build_index/src/main.rs`

- [ ] **Step 1: Write the orchestration logic**

Replace the stub `main.rs` body with this. It wires every module together.

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! `super-stt-build-index` — top-level orchestration.

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use log::{error, info, warn};

mod assets;
mod carryforward;
mod github;
mod index_json;
mod license;
mod manifest;
mod registry_toml;
mod resolve;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Path to `registry.toml` to read.
    #[arg(long, default_value = "registry/registry.toml")]
    registry: PathBuf,
    /// Path to the previously-published `index.json` (for carry-forward). If
    /// missing, falls through cleanly — bootstrap mode.
    #[arg(long)]
    prior_index: Option<PathBuf>,
    /// Where to write the new `index.json`.
    #[arg(long, default_value = "index.json")]
    out: PathBuf,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let registry_text = std::fs::read_to_string(&args.registry)
        .with_context(|| format!("reading {}", args.registry.display()))?;
    let registry = registry_toml::Registry::parse(&registry_text)?;

    let prior = match args.prior_index.as_ref() {
        Some(p) if p.exists() => {
            let text = std::fs::read_to_string(p)?;
            Some(serde_json::from_str::<index_json::Index>(&text)?)
        }
        _ => None,
    };

    let gh = github::GitHub::from_env();
    let http = reqwest::Client::new();
    let now_iso = chrono_now_iso();

    let mut out_backends: Vec<index_json::IndexBackend> = Vec::new();

    for (id, entry) in registry.0.iter() {
        if entry.removed {
            info!("skip `{id}` — removed");
            continue;
        }
        let owner_repo = owner_repo_from(&entry.repo)?;
        match build_entry(&gh, &http, id, entry, &owner_repo).await {
            Ok(b) => out_backends.push(b),
            Err(failure) => {
                error!("entry `{id}` failed: {}", failure.error);
                let prior_entry = prior.as_ref()
                    .and_then(|p| p.backends.iter().find(|b| b.id == *id));
                if let Some(carried) = carryforward::maybe_carry_forward(
                    id, prior_entry, &failure.error,
                    failure.attempted_version.as_deref().unwrap_or(""),
                    failure.attempted_tag.as_deref().unwrap_or(""),
                    &now_iso,
                ) {
                    warn!("entry `{id}` — carrying forward last-known-good (v{})", carried.version);
                    out_backends.push(carried);
                }
            }
        }
    }

    let index = index_json::Index {
        schema_version: index_json::SCHEMA_VERSION,
        generated_at: now_iso,
        min_client: index_json::MIN_CLIENT.into(),
        backends: out_backends,
    };
    let text = serde_json::to_string_pretty(&index)?;
    std::fs::write(&args.out, text.as_bytes())
        .with_context(|| format!("writing {}", args.out.display()))?;
    info!("wrote {} ({} backends)", args.out.display(), index.backends.len());
    Ok(())
}

fn owner_repo_from(repo: &str) -> anyhow::Result<String> {
    // "github.com/jorge-menjivar/super-stt" -> "jorge-menjivar/super-stt"
    let rest = repo.strip_prefix("github.com/")
        .ok_or_else(|| anyhow::anyhow!("repo `{repo}` must start with `github.com/`"))?;
    anyhow::ensure!(rest.split('/').count() == 2 && !rest.contains('/'.to_string().repeat(2).as_str()),
        "repo `{repo}` must be `github.com/<owner>/<repo>`");
    Ok(rest.to_string())
}

pub struct BuildFailure {
    pub error: String,
    pub attempted_version: Option<String>,
    pub attempted_tag: Option<String>,
}

async fn build_entry(
    gh: &github::GitHub,
    http: &reqwest::Client,
    id: &str,
    entry: &registry_toml::Entry,
    owner_repo: &str,
) -> Result<index_json::IndexBackend, BuildFailure> {
    let resolved = resolve::resolve(gh, owner_repo, entry).await
        .map_err(|e| BuildFailure {
            error: format!("{e:#}"),
            attempted_version: None,
            attempted_tag: None,
        })?;
    let attempted_version = Some(resolved.version.to_string());
    let attempted_tag = Some(resolved.tag.clone());
    let m = manifest::fetch(gh, owner_repo, entry.subdir.as_deref(), &resolved.tag).await
        .map_err(|e| BuildFailure {
            error: format!("{e:#}"),
            attempted_version: attempted_version.clone(),
            attempted_tag: attempted_tag.clone(),
        })?;
    manifest::validate(&m, &resolved.version, &entry.repo)
        .map_err(|e| BuildFailure {
            error: format!("{e:#}"),
            attempted_version: attempted_version.clone(),
            attempted_tag: attempted_tag.clone(),
        })?;

    // Decide online from any model with a non-local provider. Conservative
    // default: if any model has a provider whose name is "openai" / "mistral"
    // / "deepgram" / "anthropic", flag online.
    let online_providers: &[&str] = &["openai", "mistral", "deepgram", "anthropic"];
    let online = m.models.iter().any(|md| online_providers.contains(&md.provider.as_str()));

    let supports_gpu = m.models.iter().any(|md|
        md.supported_devices.iter().any(|d| d == "cuda" || d == "metal" || d == "rocm"));
    let supports_cpu = m.models.iter().any(|md|
        md.supported_devices.iter().any(|d| d == "cpu"));

    // Resolve and hash assets. Any error here is wrapped with the now-known
    // attempted version + tag so the carry-forward path records them.
    let wrap = |e: anyhow::Error| BuildFailure {
        error: format!("{e:#}"),
        attempted_version: attempted_version.clone(),
        attempted_tag: attempted_tag.clone(),
    };
    let mut idx_assets = index_json::IndexAssets::default();
    if let Some(wasm) = &m.assets.wasm {
        let (url, size) = assets::resolve_url(wasm, &resolved.release.assets).map_err(|e| wrap(e.into()))?;
        let sha = assets::fetch_and_validate(http, &url,
            assets::AssetExpect::Wasm { file: wasm }).await.map_err(|e| wrap(e.into()))?;
        idx_assets.wasm = Some(index_json::IndexAsset { url, size, sha256: sha });
    }
    for sa in &m.assets.subprocess {
        let (url, size) = assets::resolve_url(&sa.file, &resolved.release.assets).map_err(|e| wrap(e.into()))?;
        let sha = assets::fetch_and_validate(http, &url,
            assets::AssetExpect::Subprocess { file: &sa.file, entrypoint: &m.backend.entrypoint }).await.map_err(|e| wrap(e.into()))?;
        idx_assets.subprocess.push(index_json::IndexSubprocessAsset {
            target: sa.target.clone(), accel: sa.accel.clone(),
            cuda_major: sa.cuda_major, cuda_sm: sa.cuda_sm, cudnn: sa.cudnn,
            url, size, sha256: sha,
        });
    }

    Ok(index_json::IndexBackend {
        id: id.into(),
        source: entry.repo.clone(),
        version: resolved.version.to_string(),
        tag: resolved.tag,
        name: m.backend.name,
        description: m.backend.description,
        license: m.backend.license.unwrap_or_default(),
        kind: m.backend.kind,
        contract: m.backend.contract,
        entrypoint: m.backend.entrypoint,
        allowed_hosts: m.network.allowed_hosts,
        online, supports_gpu, supports_cpu,
        models: m.models.into_iter().map(|md| index_json::IndexModel {
            name: md.name, provider: md.provider, supported_devices: md.supported_devices,
        }).collect(),
        secrets: m.secrets.into_iter().map(|s| index_json::IndexSecret {
            name: s.name, label: s.label, required: s.required,
        }).collect(),
        options: m.options.into_iter().map(|o| index_json::IndexOption {
            name: o.name, label: o.label, r#type: o.r#type, default: o.default,
        }).collect(),
        assets: idx_assets,
        index_stale: None,
    })
}

fn chrono_now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
```

- [ ] **Step 2: Add `chrono` to `Cargo.toml`**

```toml
chrono = { version = "0.4", features = ["serde"] }
```

- [ ] **Step 3: Build it**

Run: `cargo build --release`
Expected: compiles successfully.

- [ ] **Step 4: Commit**

```bash
git add registry/scripts/build_index/
git commit -m "feat(registry/build_index): orchestration main.rs"
```

---

### Task 14: GitHub Actions workflow

**Files:**
- Create: `.github/workflows/build-index.yml`

- [ ] **Step 1: Write the workflow**

```yaml
# SPDX-License-Identifier: GPL-3.0-only
name: Build registry index

on:
  schedule:
    - cron: '0 */6 * * *'    # every 6 hours
  push:
    branches: [main]
    paths: ['registry/registry.toml']
  workflow_dispatch:

permissions:
  contents: write   # to push to gh-pages branch

concurrency:
  group: build-index
  cancel-in-progress: false

jobs:
  build:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@v6
        with:
          path: src

      - name: Checkout gh-pages (for prior index)
        uses: actions/checkout@v6
        continue-on-error: true
        with:
          ref: gh-pages
          path: pages

      - uses: dtolnay/rust-toolchain@stable

      - name: Cache cargo
        uses: actions/cache@v5
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            src/registry/scripts/build_index/target
          key: build-index-${{ hashFiles('src/registry/scripts/build_index/Cargo.lock') }}

      - name: Build indexer
        working-directory: src/registry/scripts/build_index
        run: cargo build --release

      - name: Run indexer
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          mkdir -p out
          src/registry/scripts/build_index/target/release/super-stt-build-index \
            --registry src/registry/registry.toml \
            --prior-index pages/index.json \
            --out out/index.json

      - name: Publish to gh-pages
        run: |
          cd pages 2>/dev/null || (mkdir pages && cd pages && git init -b gh-pages \
              && git remote add origin "https://x-access-token:${{ secrets.GITHUB_TOKEN }}@github.com/${{ github.repository }}.git")
          cp ../out/index.json index.json
          git add index.json
          if git diff --cached --quiet; then
            echo "index.json unchanged; nothing to publish"
            exit 0
          fi
          git -c user.name=actions -c user.email=actions@github.com \
              commit -m "Update index.json ($GITHUB_SHA)"
          git push origin gh-pages
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/build-index.yml
git commit -m "ci: workflow to build + publish registry index.json"
```

---

### Task 15: Seed `registry.toml` and `registry/README.md`

**Files:**
- Create: `registry/registry.toml`
- Create: `registry/README.md`

- [ ] **Step 1: Seed `registry/registry.toml`**

```toml
# SPDX-License-Identifier: GPL-3.0-only
# Super STT backend registry. One entry per installable backend, alphabetically
# sorted. See registry/README.md for the submission rules.

[mistral]
    repo       = "github.com/jorge-menjivar/super-stt"
    subdir     = "backends/mistral"
    tag_prefix = "mistral-"

[openai]
    repo       = "github.com/jorge-menjivar/super-stt"
    subdir     = "backends/openai"
    tag_prefix = "openai-"

[voxtral]
    repo       = "github.com/jorge-menjivar/super-stt"
    subdir     = "backends/voxtral"
    tag_prefix = "voxtral-"
```

- [ ] **Step 2: Write `registry/README.md`**

```markdown
# Super STT Backend Registry

This directory holds the source of truth for the backend catalog that ships
in the Super STT app's Download tab. A nightly GitHub Action reads
`registry.toml`, queries each entry's GitHub repo for its latest release,
validates the release's `backend.toml` and assets, and publishes a single
`index.json` to the `gh-pages` branch.

End users do not interact with this directory.

## Submitting a backend

1. Build and host your backend in your own GitHub repo. It must include a
   `backend.toml` at the chosen subdirectory (default: repo root), declaring
   `[assets.wasm]` (for wasm backends) or `[[assets.subprocess]]` (for
   subprocess backends) — see `docs/protocol/backend/config.md`.
2. Open a PR adding a new entry to `registry.toml` in **alphabetical order**:

   ```toml
   [my-backend]
   repo = "github.com/your-name/my-backend"
   ```

   Optional fields: `subdir`, `tag_prefix`, `max_version`. See the comments
   at the top of `registry.toml` and the spec at
   `docs/superpowers/specs/2026-05-29-backend-registry-design.md`.

3. Reviewers check: id is not on the reserved list (below); your repo's
   license is acceptable; you control `repo` (CODEOWNERS or a one-time
   challenge file at HEAD); and `allowed_hosts` in your `backend.toml`
   doesn't request wildcards that would be hard to vet.

4. After merge, the indexer auto-discovers releases on your repo. You
   ship new versions by tagging releases — no further PRs to this repo.

## Reserved ids

These ids are reserved for the upstream maintainers and may not be claimed
by third-party backends:

- `openai`, `anthropic`, `mistral`, `deepgram`, `voxtral`, `whisper`
- `azure`, `google`, `gcp`, `aws`, `bedrock`
- `super-stt`, `super-stt-*`

## Removing or yanking

- **Yank a specific bad version** without removing the backend: add
  `max_version = "<last-good>"` to the entry. The indexer treats anything
  above as if it didn't exist.
- **Remove the entry entirely** without giving up the id: set
  `removed = true` (keeps the row for audit; prevents squatters).
```

- [ ] **Step 3: Commit**

```bash
git add registry/registry.toml registry/README.md
git commit -m "feat(registry): seed registry.toml + submission README"
```

---

### Task 16: End-to-end integration test (mocked GitHub)

**Files:**
- Create: `registry/scripts/build_index/tests/integration.rs`

- [ ] **Step 1: Write the integration test**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end test: mock GitHub for `latest_release` + `contents` + asset
//! download; run the indexer's `build_entry` logic via the public binary.
//!
//! This is one happy-path test. Pure-unit tests cover the failure cases in
//! each module's `#[cfg(test)]` block.

use std::process::Command;

#[test]
#[ignore = "requires building the binary; run with `cargo test --release -- --ignored`"]
fn end_to_end_indexes_a_single_wasm_backend() {
    // 1. Spin up `mockito::Server` synchronously here; we drive the binary as
    //    a subprocess so the GH host can be set via env override.
    let mut s = mockito::Server::new();
    let base = s.url();

    s.mock("GET", "/repos/x/y/releases/latest")
        .with_status(200).with_body(format!(r#"{{
            "tag_name":"v1.0.0",
            "assets":[{{"name":"y.wasm","browser_download_url":"{base}/dl/y.wasm","size":4}}]
        }}"#)).create();

    s.mock("GET", mockito::Matcher::Regex(r"^/repos/x/y/contents/backend\.toml.*".into()))
        .with_status(200).with_body(format!(r#"{{
            "content":"{}",
            "encoding":"base64"
        }}"#, base64::engine::general_purpose::STANDARD.encode(MANIFEST_OK)))
        .create();

    s.mock("GET", "/dl/y.wasm").with_status(200)
        .with_body([0x00, 0x61, 0x73, 0x6d].as_slice()).create();

    let tmp = tempfile::tempdir().unwrap();
    let registry_path = tmp.path().join("registry.toml");
    std::fs::write(&registry_path, r#"
        [x-y]
        repo = "github.com/x/y"
    "#).unwrap();

    let out_path = tmp.path().join("index.json");
    let bin = env!("CARGO_BIN_EXE_super-stt-build-index");
    let status = Command::new(bin)
        .env("GITHUB_API_BASE", &base)   // see Step 3 note
        .arg("--registry").arg(&registry_path)
        .arg("--out").arg(&out_path)
        .status().expect("run binary");
    assert!(status.success());

    let text = std::fs::read_to_string(&out_path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["backends"][0]["id"], "x-y");
    assert_eq!(v["backends"][0]["version"], "1.0.0");
    assert_eq!(v["backends"][0]["assets"]["wasm"]["sha256"], "5e..."[..2]);    // sha256("\0asm")
}

const MANIFEST_OK: &str = r#"
[backend]
source = "github.com/x/y"
name = "Y"
version = "1.0.0"
kind = "wasm"
entrypoint = "y.wasm"
contract = "v1"
license = "Apache-2.0"

[assets]
wasm = "y.wasm"
"#;
```

- [ ] **Step 2: Make the GitHub base URL env-overridable**

In `src/github.rs`, change `from_env()`:

```rust
pub fn from_env() -> Self {
    let base = std::env::var("GITHUB_API_BASE").unwrap_or_else(|_| "https://api.github.com".into());
    Self::new(base, std::env::var("GITHUB_TOKEN").ok())
}
```

- [ ] **Step 3: Run the integration test**

Run: `cd registry/scripts/build_index && cargo test --release -- --ignored`
Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add registry/scripts/build_index/
git commit -m "test(registry/build_index): end-to-end integration test"
```

---

### Task 17: First-run manual validation + enable GitHub Pages

**Files:** none (operational)

- [ ] **Step 1: Trigger the workflow manually**

Push the branch to GitHub. From the Actions tab, run "Build registry index" via workflow_dispatch on `main` (or open a PR that touches `registry/registry.toml` and merge it).

- [ ] **Step 2: Inspect the workflow run**

Expected:
- Indexer prints `INFO wrote out/index.json (3 backends)`.
- A `gh-pages` branch appears (or is updated) containing `index.json`.

- [ ] **Step 3: Enable Pages on the repo**

Repo Settings → Pages → Source: "Deploy from a branch", Branch: `gh-pages`, Folder: `/`.

- [ ] **Step 4: Verify the index is live**

Run:
```bash
curl -sI https://jorge-menjivar.github.io/super-stt/index.json
```
Expected: `HTTP/2 200`. Then:
```bash
curl -s https://jorge-menjivar.github.io/super-stt/index.json | jq '.backends | length'
```
Expected: `3` (or however many entries `registry.toml` has).

- [ ] **Step 5: Document the live URL**

This is the URL Phase 2 hardcodes into the daemon.
