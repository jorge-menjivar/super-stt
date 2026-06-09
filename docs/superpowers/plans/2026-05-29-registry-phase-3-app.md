# Registry Phase 3: App Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the app's compile-time-bundled catalog with live data from the daemon's `/registry/backends`. The Install button (`Message::InstallBackend`) — currently a TODO stub — fires `POST /registry/backends/install` and subscribes to install progress events. Custom-repo button stops doing anything in the app and delegates to the daemon's install path with a `repo_url` body. After Phase 3, end-to-end install works inside the app.

**Architecture:** Delete `super-stt-app/src/daemon/catalog.rs` (the compile-time `include_str!` bundle). Add `super-stt-app/src/daemon/registry_client.rs` (thin client over the daemon's HTTP endpoints). Wire `Message::InstallBackend`, `Message::UpdateBackend`, `Message::RefreshRegistry` to the daemon. Subscribe to `registry.install.*` events and update per-card state. First-run-empty UX for when the registry is unreachable.

**Tech Stack:** Reuses the existing app stack — `cosmic`, `reqwest` (already wired to talk to the daemon via the existing `daemon::*` modules), the existing `/events` SSE consumer.

---

## File Structure

**Delete:**
- `super-stt-app/src/daemon/catalog.rs`

**Create:**
- `super-stt-app/src/daemon/registry.rs` — thin HTTP client wrappers
- `super-stt-app/src/state/registry.rs` — `RegistryState` (list cache, in-flight installs, filters)

**Modify:**
- `super-stt-app/src/daemon/mod.rs` — drop `pub mod catalog;`, add `pub mod registry;`
- `super-stt-app/src/ui/messages.rs` — replace `InstallBackend(String)` family
- `super-stt-app/src/core/app.rs` — handle new messages, subscribe to install events
- `super-stt-app/src/ui/views/models.rs` — Download tab pulls from `RegistryState`
- `super-stt-app/src/state/mod.rs` — add `registry: RegistryState`

---

### Task 1: Add the registry HTTP client to the app

**Files:**
- Create: `super-stt-app/src/daemon/registry.rs`
- Modify: `super-stt-app/src/daemon/mod.rs`

- [ ] **Step 1: Inspect the existing daemon client pattern**

```bash
ls super-stt-app/src/daemon/
```

Pick one of the existing files (e.g. `super-stt-app/src/daemon/backends.rs`) and read it. Note the http client construction pattern and how the daemon's base URL is resolved — replicate this exactly.

- [ ] **Step 2: Write `registry.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! HTTP client for the daemon's `/registry/backends` family of endpoints.
//! Mirrors the wire types in `super-stt-shared::registry`.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use super_stt_shared::registry::*;

/// Reuses the existing app-side helper for resolving the daemon base URL.
/// Look at how `super-stt-app/src/daemon/backends.rs` constructs its
/// requests — the pattern there should be cloned verbatim.
fn base_url() -> String {
    // FIXME: Replace with the project's existing helper (e.g.
    // crate::daemon::base_url() or similar). Do NOT hardcode http://localhost.
    crate::daemon::base_url()
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ListFilters {
    pub include_incompatible: Option<bool>,
    pub kind: Option<String>,
    pub online: Option<bool>,
    pub q: Option<String>,
}

pub async fn list(filters: &ListFilters) -> anyhow::Result<RegistryListResponse> {
    let mut url = url::Url::parse(&format!("{}/registry/backends", base_url()))?;
    {
        let mut q = url.query_pairs_mut();
        if let Some(b) = filters.include_incompatible { q.append_pair("include_incompatible", &b.to_string()); }
        if let Some(k) = &filters.kind { q.append_pair("kind", k); }
        if let Some(o) = filters.online { q.append_pair("online", &o.to_string()); }
        if let Some(qq) = &filters.q { q.append_pair("q", qq); }
    }
    let resp = reqwest::get(url.as_str()).await.context("GET /registry/backends")?;
    anyhow::ensure!(resp.status().is_success(), "registry list returned {}", resp.status());
    Ok(resp.json().await?)
}

pub async fn refresh() -> anyhow::Result<RefreshResponse> {
    let resp = reqwest::Client::new()
        .post(format!("{}/registry/backends/refresh", base_url()))
        .send().await.context("POST /registry/backends/refresh")?;
    anyhow::ensure!(resp.status().is_success(), "refresh returned {}", resp.status());
    Ok(resp.json().await?)
}

pub async fn install_by_source(source: &str) -> anyhow::Result<InstallAccepted> {
    let resp = reqwest::Client::new()
        .post(format!("{}/registry/backends/install", base_url()))
        .json(&serde_json::json!({"source": source}))
        .send().await.context("POST /registry/backends/install")?;
    anyhow::ensure!(resp.status().is_success(), "install returned {}", resp.status());
    Ok(resp.json().await?)
}

pub async fn install_by_repo_url(repo_url: &str) -> anyhow::Result<InstallAccepted> {
    let resp = reqwest::Client::new()
        .post(format!("{}/registry/backends/install", base_url()))
        .json(&serde_json::json!({"repo_url": repo_url}))
        .send().await.context("POST /registry/backends/install (repo_url)")?;
    anyhow::ensure!(resp.status().is_success(), "install returned {}", resp.status());
    Ok(resp.json().await?)
}

pub async fn update(source: &str) -> anyhow::Result<UpdateResponse> {
    let resp = reqwest::Client::new()
        .post(format!("{}/registry/backends/update", base_url()))
        .json(&serde_json::json!({"source": source}))
        .send().await.context("POST /registry/backends/update")?;
    anyhow::ensure!(resp.status().is_success(), "update returned {}", resp.status());
    Ok(resp.json().await?)
}

pub async fn uninstall(source: &str) -> anyhow::Result<UninstallResponse> {
    let encoded = urlencoding::encode(source);
    let resp = reqwest::Client::new()
        .delete(format!("{}/backends/{}", base_url(), encoded))
        .send().await.context("DELETE /backends/{source}")?;
    anyhow::ensure!(resp.status().is_success(), "delete returned {}", resp.status());
    Ok(resp.json().await?)
}
```

Add `urlencoding` if absent: `cd super-stt-app && cargo add urlencoding`.

- [ ] **Step 3: Wire the module + remove the old catalog**

In `super-stt-app/src/daemon/mod.rs`:

```rust
// Was: pub mod catalog;
pub mod registry;
```

Delete `super-stt-app/src/daemon/catalog.rs`.

- [ ] **Step 4: Replace catalog consumers**

```bash
grep -rn "daemon::catalog\|crate::daemon::catalog\|catalog::" super-stt-app/src/
```

For each match, replace with a call into `registry::list` (now async). For the synchronous `catalog::all()` pattern: callers need to be made async-aware, or get a cached `RegistryState` (Task 2 introduces this). For now, comment out usages and replace with `Vec::new()` placeholders to keep the build green — they'll be wired up properly in Task 5.

- [ ] **Step 5: Build**

Run: `cargo build -p super-stt-app`
Expected: compiles. Some warnings about unused symbols are fine at this stage.

- [ ] **Step 6: Commit**

```bash
git add super-stt-app/
git commit -m "feat(app/daemon): registry HTTP client; remove compile-time catalog"
```

---

### Task 2: Add `RegistryState` to the app

**Files:**
- Create: `super-stt-app/src/state/registry.rs`
- Modify: `super-stt-app/src/state/mod.rs`

- [ ] **Step 1: Inspect existing `state/` patterns**

```bash
ls super-stt-app/src/state/
```

Pick one of the existing `state/<topic>.rs` files (e.g. `state/models.rs`) and read it. Mimic its shape — usually a `#[derive(Default)] pub struct` of fields the app tracks across redraws.

- [ ] **Step 2: Write `state/registry.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! UI state for the backend registry.

use std::collections::{HashMap, HashSet};

use super_stt_shared::registry::events::{InstallError, InstallPhase};
use super_stt_shared::registry::RegistryBackend;

#[derive(Debug, Clone, Default)]
pub struct RegistryState {
    /// Last successful fetch result. `None` until the first fetch lands.
    pub backends: Vec<RegistryBackend>,
    /// `generated_at` from the last response.
    pub generated_at: Option<String>,
    /// User-facing filter state for the Download tab.
    pub filters: Filters,
    /// In-flight install state, keyed by source.
    pub installs: HashMap<String, InstallStatus>,
    /// `None` = never tried; `Some(ok)` = last refresh outcome.
    pub last_refresh: Option<RefreshOutcome>,
}

#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub include_incompatible: bool,
    /// `None` = show both wasm + subprocess.
    pub kind: Option<String>,
    /// `None` = show both online + local.
    pub online: Option<bool>,
    /// Search box content; empty = no filter.
    pub search: String,
}

#[derive(Debug, Clone)]
pub struct InstallStatus {
    pub install_id: String,
    pub phase: InstallPhase,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub error: Option<InstallError>,
}

#[derive(Debug, Clone)]
pub enum RefreshOutcome {
    Ok,
    Failed(String),
}

impl RegistryState {
    /// Map RegistryBackend.source → backend, for fast lookup.
    pub fn by_source(&self) -> HashMap<&str, &RegistryBackend> {
        self.backends.iter().map(|b| (b.source.as_str(), b)).collect()
    }

    /// Sources currently installing (used to gray out the Install button).
    pub fn in_flight_sources(&self) -> HashSet<&str> {
        self.installs.iter()
            .filter(|(_, s)| s.error.is_none() && !matches!(s.phase, InstallPhase::Rescanning))
            .map(|(k, _)| k.as_str())
            .collect()
    }
}
```

- [ ] **Step 3: Plug it into the global app state**

In `super-stt-app/src/state/mod.rs`:

```rust
pub mod registry;
pub use registry::*;
```

In the top-level state struct (search for `pub struct State` in `state/mod.rs` or wherever it's defined), add:

```rust
pub registry: registry::RegistryState,
```

Update the `Default` derive or constructor accordingly.

- [ ] **Step 4: Build + commit**

```bash
cargo build -p super-stt-app
git add super-stt-app/
git commit -m "feat(app/state): RegistryState"
```

---

### Task 3: New `Message::*` variants for the registry

**Files:**
- Modify: `super-stt-app/src/ui/messages.rs`

- [ ] **Step 1: Read the existing messages**

Read `super-stt-app/src/ui/messages.rs`. Find `InstallBackend(String)` and the messages around it.

- [ ] **Step 2: Update the message enum**

Replace `InstallBackend(String)` with the following group (keep `InstallBackend(String)` as the user-facing trigger — what the button fires — but add the daemon-feedback variants):

```rust
    /// User clicked Install on a Download-tab card.
    InstallBackend(String),
    /// User clicked Install (Custom repo) with a repo URL.
    InstallBackendFromRepoUrl(String),
    /// Daemon responded to the install request with the install_id.
    InstallAccepted {
        source: String,
        install_id: String,
        warning: Option<String>,
    },
    /// Install endpoint returned an error.
    InstallFailedToStart { source: String, error: String },
    /// SSE event: `registry.install.progress`.
    InstallProgress {
        install_id: String,
        source: String,
        phase: super_stt_shared::registry::events::InstallPhase,
        bytes_done: Option<u64>,
        bytes_total: Option<u64>,
    },
    /// SSE event: `registry.install.completed`.
    InstallCompleted { install_id: String, source: String, version: String },
    /// SSE event: `registry.install.failed`.
    InstallFailed {
        install_id: String,
        source: String,
        phase: super_stt_shared::registry::events::InstallPhase,
        error: super_stt_shared::registry::events::InstallError,
    },
    /// User clicked Update on an Installed-tab card.
    UpdateBackend(String),
    /// User clicked Uninstall on an Installed-tab card.
    UninstallBackend(String),
    /// User clicked Retry on the Download-tab empty state.
    RefreshRegistry,
    /// Result of `RegistryRefresh` (or initial list fetch).
    RegistryListLoaded(super_stt_shared::registry::RegistryListResponse),
    /// Result of `RegistryRefresh` when the fetch failed.
    RegistryListFailed(String),
    /// User typed in the Download-tab search box.
    RegistrySearchChanged(String),
    /// User toggled "Show incompatible" in the Download-tab filter row.
    RegistryIncludeIncompatible(bool),
    /// User toggled the transport filter (wasm / subprocess / both).
    RegistryKindFilter(Option<String>),
    /// User toggled the online filter (true / false / both).
    RegistryOnlineFilter(Option<bool>),
```

- [ ] **Step 3: Build + commit**

```bash
cargo build -p super-stt-app
git add super-stt-app/
git commit -m "feat(app/ui): registry install + filter messages"
```

(Build may fail because `app.rs` doesn't yet handle the new variants — that's fine, fix them in the next task. If your project's build is set to deny missing-match-arm warnings, add a temporary `_ => Command::none(),` arm in `app.rs` to keep compilation green between commits.)

---

### Task 4: Wire `Message::InstallBackend` + related to the daemon

**Files:**
- Modify: `super-stt-app/src/core/app.rs`

- [ ] **Step 1: Replace the existing stub**

Search `super-stt-app/src/core/app.rs` for `Message::InstallBackend(source)`. Currently logs "(not yet implemented)". Replace with:

```rust
            Message::InstallBackend(source) => {
                let s = source.clone();
                return Command::perform(async move {
                    crate::daemon::registry::install_by_source(&s).await
                }, move |res| match res {
                    Ok(a) => Message::InstallAccepted {
                        source: source.clone(),
                        install_id: a.install_id,
                        warning: a.warning,
                    },
                    Err(e) => Message::InstallFailedToStart {
                        source: source.clone(),
                        error: e.to_string(),
                    },
                });
            }
            Message::InstallBackendFromRepoUrl(url) => {
                let u = url.clone();
                return Command::perform(async move {
                    crate::daemon::registry::install_by_repo_url(&u).await
                }, move |res| match res {
                    Ok(a) => Message::InstallAccepted {
                        source: url.clone(),
                        install_id: a.install_id,
                        warning: a.warning.or(Some("unverified_source".into())),
                    },
                    Err(e) => Message::InstallFailedToStart {
                        source: url.clone(),
                        error: e.to_string(),
                    },
                });
            }
            Message::InstallAccepted { source, install_id, warning: _ } => {
                self.registry.installs.insert(source, super::state::registry::InstallStatus {
                    install_id,
                    phase: super_stt_shared::registry::events::InstallPhase::Downloading,
                    bytes_done: 0, bytes_total: None, error: None,
                });
            }
            Message::InstallFailedToStart { source, error } => {
                log::error!("install({source}) failed to start: {error}");
                self.registry.installs.remove(&source);
            }
            Message::InstallProgress { install_id, source, phase, bytes_done, bytes_total } => {
                if let Some(s) = self.registry.installs.get_mut(&source) {
                    if s.install_id == install_id {
                        s.phase = phase;
                        s.bytes_done = bytes_done.unwrap_or(s.bytes_done);
                        s.bytes_total = bytes_total.or(s.bytes_total);
                    }
                }
            }
            Message::InstallCompleted { install_id: _, source, version: _ } => {
                self.registry.installs.remove(&source);
                // Refresh the backends list so the new install shows up.
                return Command::perform(
                    crate::daemon::backends::list(),
                    Message::BackendsLoaded, // existing message; adjust to your project
                );
            }
            Message::InstallFailed { install_id, source, phase, error } => {
                if let Some(s) = self.registry.installs.get_mut(&source) {
                    if s.install_id == install_id {
                        s.error = Some(error);
                        s.phase = phase;
                    }
                }
            }
            Message::UpdateBackend(source) => {
                let s = source.clone();
                return Command::perform(async move {
                    crate::daemon::registry::update(&s).await
                }, move |res| match res {
                    Ok(r) if r.noop => Message::Noop, // adjust to your project's noop message
                    Ok(r) => Message::InstallAccepted {
                        source: source.clone(),
                        install_id: r.install_id.unwrap_or_default(),
                        warning: None,
                    },
                    Err(e) => Message::InstallFailedToStart { source: source.clone(), error: e.to_string() },
                });
            }
            Message::UninstallBackend(source) => {
                let s = source.clone();
                return Command::perform(async move {
                    crate::daemon::registry::uninstall(&s).await
                }, |_| Message::BackendsLoaded(vec![])); // refresh
            }
            Message::RefreshRegistry => {
                return Command::perform(crate::daemon::registry::refresh(),
                    |r| match r {
                        Ok(_) => Message::RegistryListFailed("placeholder; refreshed, now fetching list".into()),
                        Err(e) => Message::RegistryListFailed(e.to_string()),
                    });
            }
            Message::RegistryListLoaded(resp) => {
                self.registry.backends = resp.backends;
                self.registry.generated_at = Some(resp.generated_at);
                self.registry.last_refresh = Some(super::state::registry::RefreshOutcome::Ok);
            }
            Message::RegistryListFailed(err) => {
                self.registry.last_refresh = Some(super::state::registry::RefreshOutcome::Failed(err));
            }
            Message::RegistrySearchChanged(s) => self.registry.filters.search = s,
            Message::RegistryIncludeIncompatible(b) => self.registry.filters.include_incompatible = b,
            Message::RegistryKindFilter(k) => self.registry.filters.kind = k,
            Message::RegistryOnlineFilter(o) => self.registry.filters.online = o,
```

Adjust the imports + the `BackendsLoaded` / `Noop` message names to match what your codebase already uses.

- [ ] **Step 2: Trigger an initial registry fetch when the Download tab opens**

Find the place that fires on `ModelsTab::Download` activation (search for `ModelsTab::Download`). Add a `Command::perform` call that runs:

```rust
let f = self.registry.filters.clone();
Command::perform(async move {
    let filters = crate::daemon::registry::ListFilters {
        include_incompatible: Some(f.include_incompatible),
        kind: f.kind, online: f.online,
        q: if f.search.is_empty() { None } else { Some(f.search) },
    };
    crate::daemon::registry::list(&filters).await
}, |r| match r {
    Ok(resp) => Message::RegistryListLoaded(resp),
    Err(e) => Message::RegistryListFailed(e.to_string()),
})
```

- [ ] **Step 3: Build**

```bash
cargo build -p super-stt-app
```
Expected: clean build (warnings allowed).

- [ ] **Step 4: Commit**

```bash
git add super-stt-app/
git commit -m "feat(app): wire registry install + update + uninstall + refresh"
```

---

### Task 5: Download-tab UI from live data

**Files:**
- Modify: `super-stt-app/src/ui/views/models.rs`

- [ ] **Step 1: Find the existing Download-tab renderer**

Search `super-stt-app/src/ui/views/models.rs` for `catalog::all()` and the function that today renders the Download tab (look around line 920–947 per the earlier grep). That function builds one card per `CatalogBackend`.

- [ ] **Step 2: Rewrite the Download-tab renderer to consume `RegistryState`**

Replace the body of the Download-tab function so it iterates `app.registry.backends` instead of `catalog::all()`. The card shape stays the same (name, capability chips, description, Install button), with two additions:

1. **Disable Install** if `app.registry.installs` has an entry for this source without an `error`.
2. **Show progress** below the Install button when an install is in flight: phase text + `0%..100%` if `bytes_total` is known.

```rust
fn download_tab(app: &AppModel) -> Element<'_, Message> {
    use crate::state::registry::*;

    let mut col = column![];
    if app.registry.backends.is_empty() {
        col = col.push(empty_state(app));
        return col.into();
    }

    let installed = installed_sources(app); // existing helper or trivial map over app.backends

    for entry in &app.registry.backends {
        if installed.contains(&entry.source.as_str()) {
            continue; // already installed → only shows in Installed tab
        }
        col = col.push(download_card(app, entry));
    }
    col.spacing(8).into()
}

fn empty_state(app: &AppModel) -> Element<'_, Message> {
    let msg = match &app.registry.last_refresh {
        Some(RefreshOutcome::Failed(e)) =>
            format!("Couldn't reach the registry: {e}"),
        _ => "Couldn't reach the registry. Check your connection and try again.".into(),
    };
    column![
        text(msg),
        button::standard("Retry").on_press(Message::RefreshRegistry),
    ].spacing(8).into()
}

fn download_card<'a>(app: &'a AppModel, entry: &'a super_stt_shared::registry::RegistryBackend) -> Element<'a, Message> {
    let in_flight = app.registry.installs.get(&entry.source);
    let install_button = if in_flight.is_some() {
        button::standard(format!("Installing… ({})", phase_label(&in_flight.unwrap().phase)))
    } else {
        button::standard("Install").on_press(Message::InstallBackend(entry.source.clone()))
    };

    let mut col = column![
        text(&entry.name).size(16),
        text(entry.description.as_deref().unwrap_or("")).size(12),
        row![
            chip_if(entry.online, "Online"),
            chip_if(entry.supports_cpu, "CPU"),
            chip_if(entry.supports_gpu, "GPU"),
        ].spacing(4),
        install_button,
    ];

    if let Some(s) = in_flight {
        if let Some(total) = s.bytes_total {
            let pct = (s.bytes_done * 100) / total.max(1);
            col = col.push(text(format!("{pct}%")));
        }
        if let Some(err) = &s.error {
            col = col.push(text(format!("Failed: {err:?}")));
        }
    } else if !entry.compatibility.compatible {
        let reason = entry.compatibility.reason.as_deref().unwrap_or("incompatible");
        col = col.push(text(format!("Not compatible: {reason}")));
    }

    col.spacing(4).into()
}

fn phase_label(p: &super_stt_shared::registry::events::InstallPhase) -> &'static str {
    use super_stt_shared::registry::events::InstallPhase::*;
    match p {
        Resolving => "resolving",
        Downloading => "downloading",
        Verifying => "verifying",
        Extracting => "extracting",
        Installing => "installing",
        Rescanning => "finishing",
    }
}

fn chip_if<'a>(predicate: bool, label: &'a str) -> Element<'a, Message> {
    if predicate {
        widget::container(text(label)).padding(2).into()
    } else {
        widget::container(text("")).padding(0).into()
    }
}
```

Adjust `widget::container` / `column!` / `button::standard` paths to match the project's actual cosmic widget aliases. (Search the existing `models.rs` file for the patterns they're using — there will be cleaner widgets than what's written above.)

- [ ] **Step 3: Delete the now-unused imports**

Remove the `use crate::daemon::catalog;` line and any unused `Provider`, `FromStr` imports that were specific to the bundled catalog.

- [ ] **Step 4: Build**

```bash
cargo build -p super-stt-app
```

- [ ] **Step 5: Commit**

```bash
git add super-stt-app/
git commit -m "feat(app/ui): Download tab from live registry data"
```

---

### Task 6: Update + Uninstall on the Installed-tab card

**Files:**
- Modify: `super-stt-app/src/ui/views/models.rs`

- [ ] **Step 1: Find the Installed-tab card renderer**

Search `models.rs` for `installed_tab` (`pub fn` or `fn` defining the Installed tab).

- [ ] **Step 2: Add an "Update" button when registry version > installed version**

Where the existing Configure / Uninstall buttons live, add:

```rust
let installed_version = backend.version.as_str();
let registry_entry = app.registry.backends.iter().find(|b| b.source == backend.source);
let update_button = registry_entry
    .filter(|e| e.version.as_str() != installed_version)
    .map(|e| button::standard(format!("Update to {}", e.version))
        .on_press(Message::UpdateBackend(backend.source.clone()))
        .into());
```

Push `update_button` into the card's button row if it's `Some`.

Wire the existing Uninstall button (if it doesn't yet do anything) to `Message::UninstallBackend(backend.source.clone())`.

- [ ] **Step 3: Build + commit**

```bash
cargo build -p super-stt-app
git add super-stt-app/
git commit -m "feat(app/ui): Installed-tab Update + Uninstall wiring"
```

---

### Task 7: Subscribe to install progress events on `/events`

**Files:**
- Modify: `super-stt-app/src/core/app.rs` (or wherever the existing `/events` subscription is set up)

- [ ] **Step 1: Find the existing event subscriber**

Search:
```bash
grep -rn '/events\|EventSource\|sse' super-stt-app/src/
```

There will be an existing SSE consumer (used today for download progress on the in-tree models). Identify the function that decodes each event payload.

- [ ] **Step 2: Decode `registry.install.*` events**

In the event decoder, add a `match` arm for `type: "registry.install.progress" | ".completed" | ".failed"`:

```rust
match payload.get("type").and_then(|v| v.as_str()) {
    Some("registry.install.progress") => Some(Message::InstallProgress {
        install_id: payload["install_id"].as_str()?.into(),
        source: payload["source"].as_str()?.into(),
        phase: serde_json::from_value(payload["phase"].clone()).ok()?,
        bytes_done: payload["bytes_done"].as_u64(),
        bytes_total: payload["bytes_total"].as_u64(),
    }),
    Some("registry.install.completed") => Some(Message::InstallCompleted {
        install_id: payload["install_id"].as_str()?.into(),
        source: payload["source"].as_str()?.into(),
        version: payload["version"].as_str()?.into(),
    }),
    Some("registry.install.failed") => Some(Message::InstallFailed {
        install_id: payload["install_id"].as_str()?.into(),
        source: payload["source"].as_str()?.into(),
        phase: serde_json::from_value(payload["phase"].clone()).ok()?,
        error: serde_json::from_value(payload["error"].clone()).ok()?,
    }),
    _ => None, // existing match arms continue
}
```

If the existing decoder uses typed `serde_json::from_value::<EventEnvelope>` instead of manual `payload[...]` access, modify the `EventEnvelope` enum to include the registry variants by importing `super_stt_shared::registry::events::RegistryEvent` and folding its variants in.

- [ ] **Step 3: Build + smoke test**

Build:
```bash
cargo build -p super-stt-app
```

Run the app + daemon. From the Download tab, click Install on one of the three seed backends.

Expected:
- Install button changes to "Installing… (downloading)" → "verifying" → "extracting" → "installing" → "finishing" → backend disappears from Download tab and appears in Installed.

- [ ] **Step 4: Commit**

```bash
git add super-stt-app/
git commit -m "feat(app): subscribe to registry.install.* events"
```

---

### Task 8: Custom-repo button → POST with `repo_url`

**Files:**
- Modify: `super-stt-app/src/ui/views/models.rs`

- [ ] **Step 1: Find the existing "Custom repo" UI**

Search `models.rs` for "Custom repo" or "import" or "repo_url". The current button is UI-only (no Message wired to a real handler).

- [ ] **Step 2: Wire the button**

The button takes a text input for the repo URL. On click, fire `Message::InstallBackendFromRepoUrl(input_value)`. Already wired into the daemon path by Task 4. Add a small warning row below the input:

```rust
text("Unverified — only HTTPS protects this download.").size(10),
```

- [ ] **Step 3: Build + smoke + commit**

```bash
cargo build -p super-stt-app
```

Smoke: paste `github.com/jorge-menjivar/super-stt` and click Install. Expected: same install flow as the Download tab.

```bash
git add super-stt-app/
git commit -m "feat(app/ui): Custom-repo install via daemon"
```

---

### Task 9: First-run / empty-state polish + filter wiring

**Files:**
- Modify: `super-stt-app/src/ui/views/models.rs`

- [ ] **Step 1: Surface the index timestamp**

In the Download tab header, render `app.registry.generated_at` as a small "Catalog updated <relative time>" label. Use `chrono::DateTime::parse_from_rfc3339` to compute the delta.

- [ ] **Step 2: Add the filter row**

Above the Download cards list, render a row:

```rust
row![
    text("Filter:"),
    checkbox("Show incompatible", app.registry.filters.include_incompatible)
        .on_toggle(Message::RegistryIncludeIncompatible),
    pick_list(&[None, Some("wasm".into()), Some("subprocess".into())],
        Some(app.registry.filters.kind.clone()),
        Message::RegistryKindFilter),
    text_input("Search…", &app.registry.filters.search)
        .on_input(Message::RegistrySearchChanged),
    button::standard("Refresh").on_press(Message::RefreshRegistry),
].spacing(8)
```

Each filter change triggers a re-fetch of `/registry/backends` with the new query params. Wire `RegistrySearchChanged`, `RegistryIncludeIncompatible`, `RegistryKindFilter`, `RegistryOnlineFilter` in `app.rs` to fire `Command::perform(list(filters), ...)` after updating `self.registry.filters`.

- [ ] **Step 3: Build + smoke**

```bash
cargo build -p super-stt-app
```

Smoke: open Download tab, type in search, toggle filters. Each change should re-query.

- [ ] **Step 4: Commit**

```bash
git add super-stt-app/
git commit -m "feat(app/ui): registry filters + refresh + last-updated stamp"
```

---

### Task 10: Final smoke + remove the in-tree catalog tests

**Files:** various

- [ ] **Step 1: Delete catalog references**

```bash
grep -rn "catalog::\|daemon::catalog" super-stt-app/src/
```

There should be **no matches**. If there are, fix them.

- [ ] **Step 2: Remove stale tests**

If `super-stt-app/src/daemon/catalog.rs` had unit tests, they're gone with the file. Check `super-stt-app/tests/` for anything that depended on the bundled catalog and delete or rewrite.

- [ ] **Step 3: Run the workspace test suite**

```bash
cargo test --workspace --lib
```
Expected: all green. The daemon HTTP integration tests stay `--ignored` (keyring caveat).

- [ ] **Step 4: Run clippy**

```bash
cargo clippy --all-features --workspace -- -W clippy::pedantic -D warnings -D unused_must_use
```
Expected: clean.

- [ ] **Step 5: End-to-end manual run**

Run the daemon + app. Walk through:

- Download tab opens, lists the 3 seed backends from the live `index.json`.
- Click Install on `openai` (a wasm backend, smallest download). Watch progress events. Backend appears under Installed.
- On Installed tab, click Configure → set the API key. Click Select → load the model. Verify transcription works end-to-end.
- Click Uninstall. Backend disappears from Installed. Reappears in Download. No errors logged.
- Toggle "Show incompatible" — verify any backend not matching the host shows up with a reason.
- Click Refresh — fast re-fetch with no UI jitter.

- [ ] **Step 6: Final commit**

```bash
git add super-stt-app/
git commit -m "chore(app): final smoke pass for registry phase 3"
```
