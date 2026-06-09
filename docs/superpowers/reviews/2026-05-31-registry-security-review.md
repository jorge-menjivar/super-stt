# Backend Registry — Security & Correctness Review

**Date:** 2026-05-31
**Scope:** the backend registry subsystem — indexer (`registry/scripts/build_index`), daemon registry + install pipeline (`super-stt-daemon/src/registry/**`, `super-stt-daemon/src/stt_models/backends/**`, `subprocess/**`, `wasm/**`), shared wire types, and the CI workflow.
**Method:** five independent finder passes + direct re-verification against source. Line numbers are from the `modular-models` branch at review time; re-grep before editing.

## How to use this doc

Each finding has a stable ID (`REG-NN`), a severity, a verification status, exact `file:line` anchors, the failure scenario, and concrete fix guidance. Findings are mostly independent and can be fixed in parallel, with two ordering notes:

- **REG-01 is the ship-blocker** and changes the shape of `index.json` (`source` values). Land it first; it interacts with REG-03 (source identity) and with the daemon's existing `dedup_sources` guard.
- **REG-02 and REG-03** should share one `is_safe_component()` helper (see "Shared remediation" at the bottom).

### Verified clean (do NOT re-audit / do not "fix")

- All registry mutation endpoints sit behind `require_settings_scope` (`daemon/http_server.rs:495-566`). Auth is correct.
- `uninstall_backend` resolves the `{source}` param against the in-memory catalog and removes only the discovered `dir` — **not** a path built from user input (`http_server.rs:3281-3324`). No traversal here.
- `was_active` compares `active_backend` (which stores a **dir name**, set at `model_management.rs:391/490`) to `dir.file_name()`. Both are dir names — the comparison is correct.
- `toml_escape` (`install.rs:22-39`) correctly escapes `"`, `\`, and control chars; the synthesized `backend.toml` round-trips.
- `extract_tarball` first pass rejects absolute paths, `..`, and symlink entries before unpacking (`install.rs:252-268`).

---

## CRITICAL

### REG-01 — Indexer discards the validated namespaced `source`, emitting an identical `source` for every backend in a shared repo

- **Severity:** Critical (breaks the project's own seed backends; defeats the source-identity model)
- **Status:** CONFIRMED
- **Location:** `registry/scripts/build_index/src/main.rs:177`

```rust
Ok(index_json::IndexBackend {
    id: id.into(),
    source: entry.repo.clone(),   // <-- BUG: throws away m.backend.source
    ...
```

**Mechanism.** `manifest::validate` (`registry/scripts/build_index/src/manifest.rs:139-178`) deliberately permits a per-backend namespaced source (`github.com/x/repo/<name>`) and proves it is controlled by the repo owner (equals the repo, or is prefixed by `repo/`). `build_entry` then ignores `m.backend.source` and writes `entry.repo` into the index.

The seed registry (`registry/registry.toml`) has **three** backends — `mistral`, `openai`, `voxtral` — all with `repo = "github.com/jorge-menjivar/super-stt"`. So `index.json` ends up with three entries whose `source` is the identical string `"github.com/jorge-menjivar/super-stt"`.

**Failure scenario (end to end).**
1. Daemon installs all three. The installer synthesizes each `backend.toml` from `entry.source` (`install.rs:286`), so all three on-disk manifests carry the same `source`.
2. `stt_models/backends/mod.rs` discovery runs `dedup_sources`, which keeps the first-seen source and **silently drops the other two** backends.
3. `handle_set_active_backend(source)` (`model_management.rs:365`) resolves a source → first matching dir, so the user can never select a specific one of the three.

This is the same class of bug previously observed ("activating voxtral activated mistral"), now baked into the published index. The earlier daemon-side fix (distinct `source` in the in-tree `backend.toml` files) is **nullified** the moment a backend is installed from the registry, because the synthesized manifest overwrites `source` with the repo.

**Fix.**
1. In `build_entry`, set `source: m.backend.source` (validation already guarantees it is repo-or-namespaced-under-repo). Note `m.backend` is moved field-by-field below this line — reorder so `source` is read before `m.backend.name` etc., or clone it.
2. Add a uniqueness pass over `out_backends` in `main.rs` after the loop: if two entries share a `source`, fail the build (or drop the later one with a loud `error!`). Two distinct registry entries must never collide on `source`.
3. Add a regression test: two registry entries pointing at the same `repo` with distinct namespaced sources must produce two distinct `source` values in the index; identical sources must be rejected.

---

## HIGH

### REG-02 — `entrypoint` and `id` are never validated as safe path components

- **Severity:** High (arbitrary file write / arbitrary directory deletion / arbitrary host-binary execution from a registry-listed backend)
- **Status:** CONFIRMED
- **Locations:**
  - wasm copy: `super-stt-daemon/src/registry/install.rs:132` — `let dest = staging.join(&entry.entrypoint);`
  - install dir + delete: `install.rs:143-144` — `let final_path = p.backends_dir.join(&entry.id); if final_path.exists() { remove_dir_all(&final_path) }`
  - runtime spawn: `super-stt-daemon/src/stt_models/subprocess/mod.rs:111` — `let binary = backend_dir.join(&manifest.backend.entrypoint);`
  - indexer never checks it: `registry/scripts/build_index/src/manifest.rs:139-178` (validates version/source/kind/accel/license — no path check on `entrypoint`)

**Mechanism.** `entrypoint` flows from a backend's `backend.toml` → `index.json` (verbatim) → daemon `join()` calls. `PathBuf::join` with an absolute component *replaces* the base; `..` components walk up. None of the three hops validates it.

**Failure scenarios.**
- `entrypoint = "../../../../home/user/.config/autostart/x.desktop"` (or an absolute path) → `fs::copy` writes attacker-controlled wasm bytes outside the staging dir (`install.rs:132`), before any further checks.
- `entrypoint = "/usr/bin/python3"` → at runtime `binary = backend_dir.join("/usr/bin/python3") = "/usr/bin/python3"` → `systemd-run` launches an arbitrary host interpreter (still under the sandbox, but not the shipped binary).
- A registry table key like `[../../evil]` → `id = "../../evil"` → `remove_dir_all(backends_dir/../../evil)` (registry keys are human-reviewed, but the indexer should still reject them).

The registry PR reviewer only sees `repo`/`subdir`/`tag_prefix`; the `entrypoint` is fetched later from the backend's own manifest and is **not** human-gated.

**Fix.** Add a shared validator (see "Shared remediation") and apply it:
- In the indexer (`manifest::validate`): reject `entrypoint` that is absolute, empty, `.`, `..`, or contains a path separator. Same for the registry table key used as `id`.
- At install time (`install.rs`): defense-in-depth — re-validate `entry.id` and `entry.entrypoint` before any `join`, and assert the resolved path is still inside `backends_dir` / `staging` via `canonicalize` + `starts_with`.
- At runtime (`subprocess/mod.rs:111`, and the wasm equivalent): re-validate `entrypoint` before `join`.

### REG-03 — Custom-repo install trusts the remote `source` verbatim → identity spoofing, dir clobber, arbitrary-dir deletion

- **Severity:** High
- **Status:** CONFIRMED
- **Locations:**
  - `super-stt-daemon/src/registry/custom_repo.rs:114` — `source: manifest.backend.source` (no repo cross-check)
  - `custom_repo.rs:159-160` — `fn id_from_source(source) { source.rsplit('/').next().unwrap_or(source) }`
  - consumed by `install.rs:143-144`

**Mechanism.** The custom-repo path (install from an arbitrary `github.com/owner/repo` URL) does **not** run the indexer's source-vs-repo validation. It accepts whatever `source` the remote manifest declares, derives the install-dir `id` from it, and installs.

**Failure scenarios.**
- **Spoof/overwrite:** a malicious repo declares `source = "github.com/jorge-menjivar/super-stt/openai"`. `id = "openai"` → install overwrites `backends/openai` *and* the synthesized manifest makes the daemon resolve the official openai source to the attacker's backend. Silent (no spoofing warning; only the generic `unverified_source` warning that custom-repo always shows).
- **Wipe-all:** `source = "github.com/x/y/"` → `rsplit('/').next()` = `""` → `final_path = backends_dir.join("")` = `backends_dir` → `remove_dir_all(backends_dir)` deletes every installed backend, then renames staging over it.
- **Traverse:** `source = "github.com/x/.."` → `id = ".."` → operates on the parent of `backends_dir`.

**Fix.**
- Apply the `is_safe_component()` guard to `id` before it is used as a directory name (reject empty/`.`/`..`/separators).
- Require the custom-repo `source` to be consistent with the repo the user actually provided (equal to, or namespaced under, the parsed `owner/repo`), mirroring `manifest::validate`. Reject a `source` that namespaces under a *different* repo — that is the spoofing case.

### REG-04 — wasm SSRF guard: asymmetric IPv6 block + trusted backend-declared IP literals

- **Severity:** High (SSRF to internal services / cloud metadata from a wasm backend)
- **Status:** CONFIRMED
- **Location:** `super-stt-daemon/src/stt_models/wasm/host.rs:143-154`

```rust
pub(crate) fn is_disallowed_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private()
            || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),  // <-- incomplete
    }
}
```

**Mechanism.** The V6 arm omits unique-local (`fc00::/7`, `is_unique_local()`), link-local (`fe80::/10`, `is_unicast_link_local()`), and IPv4-mapped addresses (`::ffff:a.b.c.d`). An allowlisted hostname (or a DNS-rebinding response) that resolves to a v6 ULA/link-local or to `::ffff:169.254.169.254` passes the guard and connects.

Separately, `host.rs:116-121` / `:71-94` treat an **IP literal on the allowlist** as a trusted operator opt-in and skip the resolver guard entirely. But `allowed_hosts` originates from the backend's own (unreviewed) `backend.toml` → `index.json` → synthesized manifest. A malicious backend simply declares `allowed_hosts = ["169.254.169.254"]` to self-authorize the metadata endpoint.

**Failure scenario.** Malicious wasm backend with `allowed_hosts = ["169.254.169.254"]` (or a hostname that resolves to a v6-mapped internal address) issues `GET http://169.254.169.254/latest/meta-data/...` via `wasi:http/outgoing-handler`; the egress hook permits it. The wasm sandbox's only network path is this hook, so closing it here is the whole control.

**Fix.**
- Make the V6 arm mirror V4: also reject `is_unique_local()`, `is_unicast_link_local()`, and unwrap IPv4-mapped/`-compatible` addresses (`to_ipv4_mapped()` / `to_ipv4()`) and re-check them through the V4 path.
- Stop trusting backend-declared IP literals: run the disallow-list check on IP-literal allowlist entries too (a *backend* author is not the "operator"). If an operator-level escape hatch is genuinely needed, gate it on daemon config, not the manifest.
- Note the DNS TOCTOU: `check_resolved_addrs` resolves, then `default_send_request` re-resolves independently (acknowledged in the `// NOTE` at `host.rs:76`). Out of scope for a quick fix, but pinning the checked address through to connect is the real remedy.

---

## MEDIUM

### REG-05 — No download size cap and no decompression-output cap (disk-fill DoS)

- **Severity:** Medium
- **Status:** CONFIRMED
- **Locations:** `install.rs:232-250` (`stream_download` — unbounded), `install.rs:252-278` (`extract_tarball` — `archive.unpack` with no output limit). The `IndexAsset.size` field is never enforced against bytes actually received.

**Failure scenario.** A few-KB malicious `.tar.gz` decompresses to fill the disk, or a stream that never ends writes until the disk is full — DoS as the daemon user.

**Fix.** Cap streamed bytes (e.g. reject once received exceeds declared `size` + small margin; cap absolute max). In `extract_tarball`, enforce a total-output budget and a per-entry size limit while iterating.

### REG-06 — Custom-repo install skips SHA verification; base-URL overrides unvalidated

- **Severity:** Medium (by-design trust, but under-guarded)
- **Status:** CONFIRMED
- **Locations:** `install.rs:112-119` (empty `expected_sha` → warn-and-continue), `custom_repo.rs:186/203` (always `sha256: String::new()`), `super-stt-daemon/src/registry/github.rs` (`GITHUB_API_BASE` env override, no validation), `super-stt-daemon/src/registry/client.rs:68-75` (`SUPER_STT_REGISTRY_URL` override, no scheme check).

**Mechanism.** Custom-repo installs have no integrity gate beyond TLS to GitHub. Combined with an unvalidated `GITHUB_API_BASE`/`SUPER_STT_REGISTRY_URL` (accepts `http://`, no redirect policy on the client), a redirected/poisoned base can serve crafted release JSON + arbitrary asset bytes that install without a hash check.

**Fix.** Document the custom-repo trust model in `docs/protocol/endpoints/v1/registry/install.md` (it partially is). Enforce `https://` on the env overrides and set an explicit `reqwest` redirect policy (cap + same-host). Consider requiring a user-pasted expected SHA for custom installs.

### REG-07 — `tag_prefix` strip has no separator boundary (monorepo tag cross-match)

- **Severity:** Medium
- **Status:** PLAUSIBLE
- **Location:** `registry/scripts/build_index/src/resolve.rs:46` — `r.tag_name.strip_prefix(p.as_str())`

**Mechanism.** `registry_toml` forbids *identical* prefixes but not prefix-of-prefix. In a shared repo, prefixes `"a"` and `"a-"` both match tag `a-1.0.0`, so a release intended for one backend can be selected as another's "latest." A bare `"v"` prefix matches every `vX.Y.Z` tag.

**Fix.** Require the prefix to terminate at a separator (e.g. the remaining tag must start with a digit, or the prefix must end in `-`/`/`). Add `registry_toml` validation rejecting a prefix that is a prefix of another in the same repo.

### REG-08 — Carry-forward never expires

- **Severity:** Medium
- **Status:** CONFIRMED (by-design, but no bound)
- **Location:** `registry/scripts/build_index/src/carryforward.rs` + `main.rs:71-85`

**Mechanism.** On any build failure for a non-removed entry, the indexer republishes last-known-good with an `index_stale` marker and no expiry. An author can pin an old (later-found-vulnerable) version indefinitely by yanking the new release or breaking `backend.toml` so re-validation fails.

**Fix.** Record `index_stale.since` and stop carrying forward after a max staleness window (drop the entry, or surface it as unavailable). Removed entries are already dropped correctly (`main.rs:64-67`) — keep that.

### REG-09 — Panic in the background install/update task leaves the source stuck "in progress" with no terminal event

- **Severity:** Medium
- **Status:** CONFIRMED (structural)
- **Locations:** `daemon/http_server.rs:2929` (`tokio::spawn` install task) / `:3196` (update task); cleanup is the **last** statement at `:3007` / `:3263`. Panicking paths inside `install::run`: `install.rs:88` `.expect("wasm selection only when present")`, `install.rs:92` `&entry.assets.subprocess[*index]` (indexing panic).

**Mechanism.** `inflight.write().remove(...)` and the terminal `RegistryEvent` (`Completed`/`Failed`) are emitted at the end of the task body. Any panic before that point skips both → the source remains in `install_inflight` (every retry → `409`) and the app's progress card spins forever with no `Failed` event.

**Fix.** Wrap the task body so cleanup always runs: a drop-guard that removes the inflight entry and emits `Failed` on unwind, or `FutureExt::catch_unwind` around the install future. Prefer `.get(index)` over `[index]` and avoid `.expect` for selections derived from a different `entry` clone.

### REG-10 — Non-atomic install window + symlink-following local-dir copy

- **Severity:** Medium
- **Status:** CONFIRMED
- **Locations:** `install.rs:143-145` / `:201-207` (delete-then-rename), `install.rs:213-230` (`copy_dir_recursive` follows symlinks via `fs::copy`)

**Mechanism.**
- The existing backend dir is `remove_dir_all`'d *before* the `rename`. A crash in the gap leaves the backend missing (not atomic).
- `copy_dir_recursive` resolves symlinks (`DirEntry::file_type()` reports the link type, so a symlink falls into the `else` branch and `fs::copy` copies the **target** bytes). Importing an untrusted dir containing `creds -> /home/user/.ssh/id_rsa` copies that file into the backend-readable install dir (exfiltration); a symlink to a directory makes `fs::copy` error and fails the install.

**Fix.** Rename the existing dir to a `.old` sidecar, rename staging into place, then delete `.old` (so a crash leaves at most a recoverable sidecar). In `copy_dir_recursive`, reject symlink entries (`entry.file_type()?.is_symlink()`), matching the tarball path's policy.

---

## LOW / Hardening

### REG-11 — subprocess sandbox lacks resource ceilings and a transcribe timeout

- **Severity:** Low
- **Status:** PLAUSIBLE
- **Locations:** `subprocess/mod.rs:461-478` (`hardening_params` — no `MemoryMax`/`TasksMax`/`CPUQuota`, no `SystemCallArchitectures=native`), `subprocess/mod.rs:205-234` (`request`/`send_request` — no timeout)

**Mechanism.** `wait_for_ping` (30s) and `load` (10min) are bounded, but `transcribe_audio`'s `request()` is not — a backend that accepts the connection and never responds wedges the call. No cgroup memory/task ceiling, so a malicious/buggy backend can OOM or fork-bomb the host. (`PrivateNetwork`, `ProtectSystem=strict`, `NoNewPrivileges`, `DevicePolicy=closed` are solid.)

**Fix.** Add `MemoryMax=`, `TasksMax=`, `SystemCallArchitectures=native` to `hardening_params`; wrap per-request I/O in `tokio::time::timeout`.

### REG-12 — Weak tag↔manifest version binding

- **Severity:** Low
- **Status:** PLAUSIBLE
- **Location:** `registry/scripts/build_index/src/manifest.rs:140`

`Version::parse(m.backend.version.trim_start_matches('v'))` strips *all* leading `v`s (`"vv1.0.0"` → `1.0.0`), and semver equality may ignore build metadata, so `version = "1.0.0+anything"` can pass against tag `v1.0.0`. Tighten to a single optional `v` and compare the exact normalized string against the resolved tag.

### REG-13 — CI workflow: write token shares a job with indexer build/run; token in remote URL

- **Severity:** Low
- **Status:** PLAUSIBLE
- **Location:** `.github/workflows/build-index.yml`

The job holding `contents: write` also compiles and runs the indexer and embeds `GITHUB_TOKEN` in the remote URL written to `pages/.git/config`. Split into an unprivileged build/index job (emits `index.json` as an artifact) and a privileged publish job (consumes the artifact, holds the token). Avoid persisting the token in `.git/config`.

### REG-14 — Registry URL / disk cache are unauthenticated

- **Severity:** Low (same-user; defense-in-depth)
- **Status:** PLAUSIBLE
- **Locations:** `client.rs:68-75` (`SUPER_STT_REGISTRY_URL` accepts `http://`, no redirect policy), `client.rs:164-172` (`load_from_disk` trusts the cache with no integrity binding)

Enforce `https://` (allow `http://localhost` only for tests behind a flag), set an explicit redirect policy, and consider binding the cache file to the URL it came from so a stale/cross-URL cache isn't served.

---

## Shared remediation

Several findings (REG-02, REG-03, and the dir-name uses in `install.rs`/`custom_repo.rs`/`local_dir.rs`) want one helper. Suggested shape:

```rust
/// A backend `id` or `entrypoint` must be a single, relative, non-traversing
/// path component. Reject empty, ".", "..", absolute paths, and anything
/// containing a path separator (or a NUL).
fn is_safe_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
}
```

Apply it (a) in the indexer's `manifest::validate` for `entrypoint` and the registry table key, (b) at install time for `entry.id` and `entry.entrypoint` before any `join`, and (c) at runtime in `subprocess/mod.rs` (and the wasm loader) before `backend_dir.join(entrypoint)`. For the install dir specifically, additionally `canonicalize` the result and assert `starts_with(backends_dir)` as belt-and-suspenders.

## Suggested fix order

1. **REG-01** (changes index shape; coordinate with `dedup_sources`).
2. **REG-02 + REG-03** (shared `is_safe_component` helper).
3. **REG-04** (SSRF arm + IP-literal trust).
4. **REG-05, REG-09, REG-10** (DoS / stuck-state / atomicity — independent, parallelizable).
5. Remaining MEDIUM/LOW as capacity allows.
