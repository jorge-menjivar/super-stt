# Code Quality Audit — July 2026 (follow-up)

Second full-workspace review, conducted after the ~20 refactor PRs that resolved the
first audit (`docs/audits/2026-07-code-quality.md`). That audit's 76 items were all
re-verified against the current tree first (see *Prior-audit verification* below); this
document contains only **new** findings — issues the first pass did not cover, plus a
handful of regressions the refactors themselves introduced. File:line references are to
`a8fe0b60`. Checkboxes track resolution.
Severity: 🔴 high · 🟠 moderate · 🟡 minor.

Method: multi-agent sweep across 19 dimensions (concurrency, security, error handling,
protocol conformance, blocking work, resource lifecycle, tests/CI, duplication,
crate/API design, app architecture, applet/visualization, deps/build, performance, docs,
i18n, install scripts, consent UX, CLI surface, plus a build/lint/test gate run). Each
candidate was checked by an independent refuter (does the defect exist in the code?) and
a novelty checker (is it already covered or deferred in the first audit?); only findings
surviving both are listed. 64 confirmed, 6 rejected.

---

## Prior-audit verification

All **76** items in `2026-07-code-quality.md` (Tier 1 #1–31, Tier 2 #1–8, Tier 3 #1–37)
were verified **fixed** in the current tree — each fix located in code, every sub-point
either addressed or covered by a named deferral, and no regression of the fix itself.
Deleted-code claims were confirmed by zero remaining production references
(`RealTimeTranscriptionManager`, `start_realtime`/`realtime_audio`, `get_active_sessions`,
`add_audio_chunk`, the substring status matcher, `install_mock_if_requested`, etc.);
added-test claims were confirmed present. Nothing from the first audit needs rework.

Note that several *new* findings below are the same class of problem the first audit
fixed in one place, recurring in a place it did not enumerate (e.g. the write-only
`last_udp_data` watchdog it deleted from the applet still exists in the app; the wire-enum
snake_case normalization it applied to daemon/CLI enums was never applied to the four
applet visualization enums). Those are genuinely new sites, not regressions.

---

## Systemic themes

Five clusters account for most of the findings:

1. **The `just install*` path is broken and has diverged from the shell installers.**
   The generated `stt` wrapper execs the daemon binary (which has no `record`
   subcommand), so the flagship `Super+Space → stt record --write` shortcut fails; the
   `install-app` desktop-file path points at a file that was renamed away; the
   `--model` sed targets a `--socket` flag that doesn't exist; and the `stt`-group
   provisioning grants nothing. The `scripts/install-*.sh` beta path does most of this
   correctly, so the two supported install routes disagree. **(Tier 1 #1–2, Tier 2 #7–9)**

2. **`SECURITY.md` and `README.md` describe the pre-refactor product.** The security doc
   details an SO_PEERCRED/binary-verification auth model and an `stt`-group socket ACL —
   the daemon uses neither (it's session-token + scope + consent, same-uid only). The
   README documents a `--write-method` flag and `--stop-mode manual` value that clap
   rejects, and an "Online Models" sidebar page that no longer exists. Operational
   commands name a `super-stt` binary and `--socket` flag that don't exist. **(Tier 2
   #1–6)**

3. **The prior audit's error-code / wire-contract unification is incomplete.** `error_code`
   is absent from the registry, backend-option, secret, and model-language error envelopes
   despite `transport.md` promising it "on every error"; the `OnlineModelsDisabled`,
   `InvalidDevice`, and `CudaUnavailable` codes are defined but never produced (uncoded
   500s ship where the docs pin 400s); `daemon_status_changed` is still an untyped
   hand-matched JSON contract whose keys have already drifted (`to_device` vs
   `target_device`); `MAX_MANIFEST_BYTES` is triplicated. The machinery landed; not every
   producer was migrated onto it. **(Tier 2 #10–13)**

4. **The Tier 3 #35 async `SessionPersister` rewrite introduced a durability + ordering
   regression.** Snapshots are captured under the lock but `submit()`ed after it's
   released, so channel order can invert lock order and the coalescer writes a stale
   snapshot — a revoked token resurrects after restart. The task is never drained on
   shutdown, so a token minted/revoked in the final second is lost. The channel is
   unbounded. **(Tier 1 #4–5)**

5. **Blocking work the first audit didn't reach, plus real-time-thread allocations.** The
   record-start path runs the full cpal cold-start and a `std::thread::sleep` verification
   spin on a runtime worker; the default XDG-portal typing backend rebuilds a `zbus::Proxy`
   per keysym; the cpal input callback clones the mono buffer on the audio RT thread every
   chunk; SSE fan-out serializes each frame twice. **(Tier 1 #3, Tier 3 #1–4)**

---

## Tier 1 — defects worth fixing

### [x] 1. 🔴 Install: `just install` builds a `stt` wrapper that execs the daemon, so `stt record --write` fails

- **Where:** `justfile:503` writes the wrapper as `exec {{ daemon_dst }} "$@"`
  (→ `super-stt-daemon`); the recipe installs a COSMIC shortcut spawning
  `stt record --write` (`justfile:517`).
- **Problem:** the daemon's clap surface (`super-stt-daemon/src/cli.rs`) defines no
  `record` subcommand — its own help text says *"Use `super-stt-cli` (or the `stt`
  wrapper) to drive recordings."* `record` lives in `super-stt-cli`
  (`src/main.rs:53,104`), which *is* installed but the wrapper never points at it.
  (The wrapper comment at `justfile:500` also mislabels it as invoking the daemon
  "directly", compounding the confusion.)
- **Impact:** every `just install` / `just install-daemon` user gets a broken Super+Space
  shortcut and a broken `stt` command — the primary record-and-type workflow fails with a
  clap "unexpected argument 'record'" error. `scripts/install-beta.sh:251` does this
  correctly (`exec super-stt-cli`), so the two install paths diverge.
- **Fix:** change `justfile:503` to `exec {{ cli_dst }} "$@"` and have `install-daemon`
  run `install-cli`.
- **Resolved (branch `refactor/audit2-cleanup`):** the wrapper now execs `{{ cli_dst }}`
  (`super-stt-cli`, which owns the `record` subcommand) and its comment says so;
  `install-daemon` now runs `install-cli` before writing the wrapper (mirroring the
  bundled consent-helper step), so the target binary always exists. Dropped the
  now-redundant `install-cli` call from the top-level `install` recipe.

### [x] 2. 🔴 Protocol: `POST /v1/transcribe` silently discards documented `audio_data`/`sample_rate`/`language`; pre-captured-audio requests open the mic instead

- **Where:** the handler builds its command via `build_request("record", data)`, and
  `build_request` hard-codes `audio_data: None`, `sample_rate: None`, `language: None`
  (`super-stt-daemon/src/daemon/http/internal/helpers/dispatch.rs:14-23`), stuffing the
  whole body into `.data`.
- **Problem:** `transcribe.md` (12-17, 33-43) and `transport.md` (46-70) document
  top-level `audio_data`/`sample_rate` (pre-captured one-shot) and a per-request
  `language` override. The `record` command reads only write/stop/wait/preview; the
  `transcribe` command that consumes `audio_data` has **no** HTTP route (every HTTP
  dispatch uses `"record"`).
- **Impact:** a spec-conformant client sending `{audio_data:[…], sample_rate:16000}` for
  offline transcription of a supplied buffer is ignored — the daemon opens the live
  microphone and records instead (unexpected, privacy-relevant). The documented
  per-request `language` override is also silently inert on this endpoint.
- **Fix:** branch in the transcribe handler on `audio_data` presence → dispatch the
  `transcribe` command with the buffer populated, and thread `language` through
  `cmd_record`; or delete the three fields from the docs and the dead `cmd_transcribe`
  HTTP contract. `build_request` must stop unconditionally nulling them for this route.
- **Resolved (branch `refactor/audit2-cleanup`):** implemented the documented behavior
  (the daemon's offline `handle_transcribe` already existed and was wired to
  `Command::Transcribe`; only the HTTP handler never reached it). `POST /v1/transcribe`
  now branches on a top-level `audio_data` array → `build_transcribe_request` (moves the
  buffer out, reads `sample_rate`/`language`) → offline transcription → `200 JSON
  {status, transcription}`, never touching the mic; a bad `audio_data` is `400`, and
  `audio_data` + `stream_realtime` is rejected `400 stream_realtime_with_audio_data`. The
  mic SSE path is unchanged (extracted into `transcribe_mic`). The per-request `language`
  override (previously inert) is threaded through both transcription paths; per the
  transcription-language design an explicit request language is passed straight through to
  the backend (not re-adapted against the model's supported set) — a review-caught
  precedence bug in the first attempt, now fixed. Added unit tests for the command
  language-threading and `build_transcribe_request`. **Out of scope (Tier 2 #8):** the
  mic-path response shapes (202 fire-and-forget, `stream_realtime`-gates-preview) are a
  separate finding and were left behavially unchanged.

### [ ] 3. 🟠 Daemon: record-start parks a tokio worker on cpal enumeration + a `std::thread::sleep` verification spin

- **Where:** `setup_recording_session` (async, awaited from the `POST /v1/transcribe`
  task) → `DaemonAudioRecorder::new_with_theme` (`recorder.rs:194`) →
  `warm_up_audio_system` → cpal `default_host/default_output_device/default_output_config`
  (`device.rs:126`) and `attempt_device_verification`, whose readiness check is a busy
  `std::thread::sleep(10ms)` loop bounded by `DRIVER_INIT_TIMEOUT`=500ms plus
  `sleep(50*attempt)` retries (`device.rs:176,246,249`). `spawn_recorder` also calls
  `detect_default_input_sample_rate` on the async worker (`preview.rs:52`).
- **Problem:** none of this is on `spawn_blocking`. Tier 3 #4 offloaded the beep and
  request-handler keyring but left the surrounding cpal init in place.
- **Impact:** on a cold start (and every start >30s after the last, per
  `DEVICE_CACHE_VALIDITY`) a runtime worker is blocked for up to ~1.6s of real
  `std::thread::sleep`, degrading concurrent SSE/event/status handling for widgets and the
  applet exactly when the user starts talking.
- **Fix:** wrap recorder construction and `detect_default_input_sample_rate` in
  `spawn_blocking` (mirror `handle_get_gpu_info`), or fold setup into the already-spawned
  blocking recorder task; replace the `std::thread::sleep` spin with an off-runtime wait.

### [ ] 4. 🟠 Daemon: `SessionPersister` submits snapshots after releasing the lock — a revoked token can resurrect across restart *(security)*

- **Where:** `mint` (`tokens.rs:315-319`), `validate`-eviction (`:329-342`), and `revoke`
  (`:354-362`) build the snapshot under the `inner` Mutex, then call
  `self.persist.submit(snapshot)` **after** the guard drops. `persist_loop` coalesces the
  backlog to the last-received snapshot (`:96-103`), justified by "older snapshots are
  strict subsets of the newest".
- **Problem:** that invariant holds only if channel order matches lock-acquisition order.
  Because `submit()` runs off-lock, two operations that serialize correctly on the Mutex
  can invert their submit order (T1 mutates+drops, is preempted before submit; T2
  mutates+drops+submits; T1 resumes and submits its older snapshot). The coalescer then
  writes the stale, smaller snapshot to the keyring.
- **Impact:** with `mint(C)`-then-`revoke(B)`, the channel can end `[{A,C},{A,B,C}]` and
  persist writes `{A,B,C}` — a token revoked on `exe_changed` (a binary-swap / potential
  compromise signal) reappears valid after the next restart within its 30-day TTL. The
  mirror case silently drops a just-minted session from disk. This is exactly the race
  Tier 3 #35 introduced the funnel to eliminate.
- **Fix:** move `submit()` inside the locked block (`UnboundedSender::send` is
  non-blocking, cannot deadlock under the std Mutex), so channel order == lock order; or
  have the persist task read the authoritative map under the same lock instead of
  accepting caller-captured snapshots. A monotonic seq stamped under the lock + keep-highest
  in the loop also works.

### [ ] 5. 🟠 Daemon: session-token persistence is never drained on shutdown (durability regression from the async rewrite)

- **Where:** `mint`/`validate`/`revoke` call `self.persist.submit(snapshot)` →
  `tx.send()` into an unbounded channel drained by `persist_loop` (`tokens.rs:96-103`).
  `daemon_main::run`'s shutdown calls `shutdown_unload().await`, sleeps a fixed 100ms, then
  `std::process::exit(0)` (`daemon_main.rs:183-189`) — dropping the channel and killing the
  task with queued snapshots unwritten.
- **Problem:** Tier 3 #35 moved persistence from a synchronous inline flush (token on disk
  before `mint` returned) to fire-and-forget. Nothing awaits the task on shutdown.
- **Impact:** a token minted (or revoked via the `/events` exe-check) in the sub-second
  window before shutdown is never written. After restart a just-minted token is missing
  (silent re-auth popup); a just-revoked token re-validates from disk until the next
  exe-check re-revokes it. The 100ms grace is not a guarantee — the `spawn_blocking`
  keyring write can block on a D-Bus unlock prompt far longer.
- **Fix:** give `SessionPersister` an explicit flush/close handshake (keep a `JoinHandle`
  or flush-sentinel + oneshot ack); in the shutdown path drop the submit handles and await
  the persist task (bounded by a timeout) before `process::exit`.

### [ ] 6. 🟠 Daemon: runtime env bypasses (`SUPER_STT_AUTO_APPROVE`, `SUPER_STT_KEYRING_MOCK`) are honored in release builds *(security)*

- **Where:** `auth_request` reads `AUTO_APPROVE_ENV` with a plain runtime
  `std::env::var(...)` (`http/v1/auth/request.rs:154`) and, when set, skips
  `ask_user_for_consent` and mints a full-scope token with no popup.
  `keyring::mock_store()` activates a process-global in-memory secret/session store
  whenever `SUPER_STT_KEYRING_MOCK` is merely present (`keyring.rs:81`, `var_os().is_some()`).
- **Problem:** neither is gated. The first audit's #30 explicitly compiled the analogous
  consent-timer bypass out of release via `#[cfg(debug_assertions)]` (+ a
  `#[cfg(not(...))]` no-op stub, `consent/main.rs:321,347`); these two sibling bypasses in
  the daemon were left as live runtime checks.
- **Impact:** in a shipped binary, one stray/injected env var defeats the human consent
  gate silently, or reroutes all backend API keys and the session store to a
  non-persistent plaintext in-process map with no encryption at rest.
- **Fix:** gate both behind `#[cfg(debug_assertions)]` with release no-op stubs (as #30
  did), or behind a dedicated `test-hooks` cargo feature the integration tests enable.

### [ ] 7. 🟠 Daemon: realtime WebSocket bridge feeds the guest through an unbounded channel with no backpressure; sessions are unbounded

- **Where:** `run_realtime_session` creates the incoming bridge as
  `mpsc::unbounded_channel` (`http/v1/transcribe.rs:47`); `relay_in` (`:63-78`) forwards
  every socket frame with no capacity await; the idle watchdog (`:114-121`) only fires
  after `REALTIME_IDLE_TIMEOUT` of **no** frames; the session holds only
  `daemon.model.read()` (`:110`), a shared lock.
- **Problem:** a client producing audio frames faster than the guest drains them grows the
  channel without bound, and the shared read lock lets N clients open N concurrent
  sessions each with its own unbounded buffer.
- **Impact:** memory-exhaustion DoS reachable by any authorized transcribe-scope client
  (gated behind `wasm-backends`). A client that keeps sending never trips the idle
  watchdog.
- **Fix:** use a bounded mpsc and let `relay_in` apply backpressure (await send, or
  drop-oldest on `Full`); cap concurrent realtime sessions with a try-acquire semaphore.

### [ ] 8. 🟠 App: `fetch_current_model` error path bypasses the epoch guard and destructively clears live model state

- **Where:** `fetch_current_model` tags success with `current_model_epoch` and
  `CurrentModelLoaded` drops a stale snapshot (`handlers/model.rs:93`), but the **error**
  branch → `ModelError` → `set_model_error` → `clear_loaded_model()` + banner
  (`:123-145`) has no epoch check. It's issued on every `EventStreamConnected`
  (`daemon/mod.rs:36`) — the reconnect path where transient errors and a concurrent live
  `model_switched` are most likely.
- **Impact:** a transient `get_current_model` failure that resolves after a live
  `model_switched` set the correct model wipes it to idle and raises a model-error banner —
  the exact clobber the epoch guard exists to prevent, contradicting freshly-arrived
  authoritative state.
- **Fix:** thread the captured epoch into the error branch; drop (or only log) the failure
  when the epoch has advanced — never `clear_loaded_model()` on a stale-epoch fetch error.

### [ ] 9. 🟠 Daemon: online-model rejection returns uncoded 500 instead of the documented `400 online_models_disabled`

- **Where:** the gate at `model_management/switch.rs:209-213` returns
  `DaemonResponse::error("Online models are disabled…")` with no `error_code`;
  `status_code_for_response` maps an uncoded error to `ErrorCode::Internal` → 500.
  `ErrorCode::OnlineModelsDisabled` (a 400 code) exists but has zero producers.
- **Impact:** `active_model.md:82` pins `400 online_models_disabled`; clients get a 500
  with no machine-readable code, indistinguishable from a crash, so they can't show the
  "enable online models" affordance and may retry a permanent failure. The variant added by
  Tier 2 #3 is dead.
- **Fix:** emit `DaemonResponse::error_with_code(ErrorCode::OnlineModelsDisabled, …)` at
  `switch.rs:210`.

### [x] 10. 🟠 Install: `just install-daemon --model X` silently drops the model (sed targets a nonexistent `--socket` flag)

- **Where:** `justfile:485` runs
  `sed -i "s|--socket %t/stt/super-stt.sock|… --model $model|"` on the installed unit.
- **Problem:** the packaged unit's ExecStart is `…/super-stt-daemon` with **no** `--socket`
  argument (`systemd/super-stt.service`), so the substitution matches nothing; the daemon
  also accepts no `--socket` flag (`cli.rs`) and the real socket is `super-stt-http.sock`.
- **Impact:** `just install-daemon --model whisper-large` installs a unit that ignores the
  requested model; the daemon starts with the saved/default preference, no error.
- **Fix:** append `--model $model` to the actual `super-stt-daemon` ExecStart line
  (the daemon reads `-m/--model`); drop the obsolete `--socket` text.
- **Resolved (branch `refactor/audit2-cleanup`):** the sed now targets the real
  `ExecStart=%h/.local/bin/super-stt-daemon` line and appends `--model $model` (verified
  against the packaged unit); the obsolete `--socket` pattern is gone.

---

## Tier 2 — contract & documentation drift

### [ ] 1. 🟠 Docs: `SECURITY.md` describes an obsolete SO_PEERCRED/binary-verification auth model and omits the actual token/scope/consent model

- **Where:** `docs/SECURITY.md:59-72,159` ("Write mode requires verification that the
  client is the legitimate stt binary", "Unix peer credentials…", "Binary verification…",
  "Process impersonation … verification").
- **Problem:** the real model (`docs/protocol/auth.md`, `daemon/http/v1/mod.rs` middleware)
  is per-request Bearer session tokens carrying user-approved scopes, minted via a one-time
  consent popup bound to `/proc/<pid>/exe`. `SECURITY.md` never mentions tokens, scopes,
  the consent flow, or `super-stt-consent`. It also asserts "No remote access: Network
  connections impossible" (`:67`) while its own later section documents outbound HTTPS to
  the registry/GitHub (`:98-121`).
- **Impact:** the one security doc a reviewer/deployer consults describes an authorization
  mechanism the product no longer uses and omits the one it relies on.
- **Fix:** rewrite the auth sections to describe the token/scope/consent model
  (cross-ref `auth.md`), keep SO_PEERCRED only in its real role (resolving the peer exe for
  the prompt + the uid-mismatch check), and reconcile the "No remote access" claim.

### [x] 2. 🟠 Security/Install: the `stt`-group access-control model is documented and provisioned by installers but does not exist in the daemon

- **Where:** `scripts/setup-stt-group.sh:58-61` (creates the group, prints "Only members
  of 'stt' group can connect"), `install-stable.sh:198-208`, `SECURITY.md:20-55`
  ("Group-Based Access Control"), `README:244`.
- **Problem:** the daemon binds the socket 0o660 owned by the process's **primary** group
  and never chgrps it to `stt` nor checks membership (`http/server.rs:88-93`); the listener
  lives under a per-user 0700 `$XDG_RUNTIME_DIR/stt/` dir and enforces same-uid only
  (`request.rs:58-72`). `just install-app`→`install-daemon` still invokes
  `setup-stt-group.sh` (`justfile:446-449`) despite the same recipe's comment saying the
  group is no longer required. (`SECURITY.md:53` also names the socket `super-stt.sock` vs
  the real `super-stt-http.sock`.)
- **Impact:** users are prompted for sudo to create a group that grants nothing, and told
  unauthorized local users are blocked by a mechanism that isn't implemented — a misleading
  security claim.
- **Fix:** delete `setup-stt-group.sh` and its `justfile`/`install-stable.sh` invocations;
  rewrite the `SECURITY.md`/`README` group section to describe the real same-uid + consent
  model and the correct socket name.
- **Resolved (branch `refactor/audit2-cleanup`):** deleted `scripts/setup-stt-group.sh` and
  removed the `install-daemon` invocation. Rewrote `SECURITY.md`'s socket/group sections
  (Socket Access Control, Best Practices, Threat Model, Incident Response, Process Isolation,
  Deployment Checklist) to the real per-user model — owner-only `$XDG_RUNTIME_DIR`, same-UID
  peer check, consent-token authorization, correct `super-stt-http.sock` name — and updated
  the README troubleshooting note. **Scoped out deliberately:** `install-stable.sh`'s own
  inline `setup_stt_group` is left intact — that script installs the *legacy* single
  `super-stt` binary whose `sg stt -c` wrapper genuinely needs the group; touching it would
  break legacy installs. The broader SO_PEERCRED auth-model rewrite (Tier 2 #1) and the
  `super-stt --socket` systemd example (Tier 2 #3) are separate findings, untouched here.

### [x] 3. 🟠 Docs: `SECURITY.md` operational commands reference a nonexistent `super-stt` binary, `--socket` flag, and wrong socket filename

- **Where:** `docs/SECURITY.md:239` (`ExecStart=%h/.local/bin/super-stt --socket
  %t/stt/super-stt.sock`), `:140,143` (`cargo run --bin super-stt`), `:53` (socket path).
- **Problem:** the daemon binary is `super-stt-daemon`; `super-stt` is the removed legacy
  binary. The clap surface defines only `--model/--device/--verbose/--audio-theme` — no
  `--socket` (path comes from `get_http_socket_path`/`SUPER_STT_HTTP_SOCKET`). The socket is
  `super-stt-http.sock` under `$XDG_RUNTIME_DIR/stt/` (`validation/paths.rs:60`).
- **Impact:** a user applying the "Recommended Systemd Hardening" unit gets a service that
  fails to start; the socket-verification command inspects a path that never exists.
- **Fix:** `super-stt` → `super-stt-daemon`, drop `--socket` (or document
  `SUPER_STT_HTTP_SOCKET`), fix the socket path.
- **Resolved (branch `refactor/audit2-cleanup`):** the systemd `ExecStart` is now
  `%h/.local/bin/super-stt-daemon` (no `--socket`, matching the packaged unit), and both
  `cargo run --bin super-stt` dev lines are `super-stt-daemon`. The `:53` socket-verification
  path was already corrected to `super-stt-http.sock` in the Tier 2 #2 rewrite. Also fixed the
  same wrong-socket-name (`super-stt.sock`) in `AGENTS.md`. The `systemctl`/`journalctl -u
  super-stt` commands are left as-is — `super-stt` is the correct *unit* name (`service_name`),
  distinct from the `super-stt-daemon` binary. **Out of scope (flagged, not a #3 item):** the
  "Recommended Systemd Hardening" block still lists `ProtectHome=true`/`ProtectSystem=strict`,
  which are incompatible with a `--user` service whose binary lives in `~/.local/bin` and whose
  socket lives in `/run/user/<uid>`; making that block actually startable is a separate fix.

### [ ] 4. 🟠 Docs/CLI: README advertises a `--write-method` flag and `--stop-mode manual` value the CLI rejects

- **Where:** `README.md:83` (`stt record --write --stop-mode manual --write-method
  ydotool`), `:61,80-84`.
- **Problem:** the `record` subcommand (`super-stt-cli/src/main.rs:52-79`) defines only
  `--write`, `--wait`, `--stop-mode`; there is no `--write-method` (and `TranscribeOptions`
  carries no write-method, so it can't be set per-request by any client). `--stop-mode`
  values come from `RecordingStopMode::WIRE_VARIANTS`
  (`silence_only|silence_and_manual|manual_only`); the alias `manual` was deliberately
  dropped by Tier 2 #2.
- **Impact:** copy-pasting the documented command for the primary use case gives two hard
  clap errors before recording starts.
- **Fix:** `--stop-mode manual` → `manual_only`, delete `--write-method ydotool`, and state
  that write method is app/config-only (not a per-recording CLI flag) — or add it to
  `TranscribeOptions` + clap if per-recording override is intended.

### [ ] 5. 🟡 Docs: README "Enabling Online Models" references an "Online Models" sidebar page that no longer exists

- **Where:** `README.md:111-117` ("Navigate to Online Models in the sidebar…").
- **Problem:** the sidebar now has Models, Library, Customization (`init.rs:21-33`); online
  providers are installed as backends and configured with API keys via the Library /
  Configure-sheet secret flow (`ui/messages.rs:263-294`), not a dedicated page.
- **Fix:** update the section to the current IA (install the provider from Library/Browse,
  open its Configure sheet for the API key) and correct the sidebar page names.

### [ ] 6. 🟠 Protocol: `error_code` is absent from the registry, backend-option, secret, and model-language error envelopes

- **Where:** `backends/mod.rs:28-35` (`json_error` → `{status:"error", message:<code>}`,
  no `error_code`) covers option/secret/model-language; `registry/mod.rs:21-39`
  (`registry_error` → `{error:<code>}`, no `status`, no `error_code`). Only the
  `DaemonResponse` path attaches `error_code`.
- **Problem:** `transport.md:243-252` states `error_code` "is present on every error the
  daemon originates" and tells clients to switch on it, not on `message`. Two whole
  endpoint families put the machine-readable identifier only in `message` (or a third
  envelope shape).
- **Impact:** a generic client following the contract has no machine-readable signal for
  the entire registry/options/secrets/model-language surface. The Tier 1 #5 / Tier 2 #3
  envelope unification is incomplete.
- **Fix:** route these errors through the `error_code` envelope (or add `error_code` +
  `status:"error"` to `json_error`/`registry_error`); update the per-endpoint error tables.

### [ ] 7. 🟠 Protocol: `active_device` error contract has drifted from the ErrorCode enum and the daemon

- **Where:** `device_management.rs:131-135`; `active_device.md:66-69`.
- **Problem:** (1) an unrecognized device is rejected with an uncoded error → 500, but the
  doc pins `400 invalid_device` and **no** `InvalidDevice` variant exists. (2) the validator
  accepts only `cpu`/`cuda`, yet the doc (and the GET response comment) lists `metal`, so a
  documented-valid `metal` request is 500'd. (3) the doc pins `400 cuda_unavailable` and
  `ErrorCode::CudaUnavailable` exists, but it has zero producers — the daemon silently falls
  back to CPU (`:406-409`), so that path never fires.
- **Impact:** every branch of the documented device error contract is unfulfilled; clients
  can't distinguish bad-request from server-failure for the GPU toggle.
- **Fix:** add an `InvalidDevice` (400) variant and emit it; reconcile the accepted-device
  set with the doc (add `metal` or drop it); either produce `cuda_unavailable` or document
  the silent-CPU-fallback (the doc currently self-contradicts).

### [x] 8. 🟠 Protocol: `POST /transcribe` response shapes and the `stream_realtime` field diverge from the docs

- **Where:** the handler unconditionally returns 200 `text/event-stream`
  (`http/v1/transcribe.rs:353-358`); `stream_realtime` is never read; preview is gated on
  `preview`/`preview_typing_enabled` (`recording/preview.rs:136-142`).
- **Problem:** `transcribe.md`/`transport.md` promise 202 JSON for fire-and-forget, 200
  JSON for pre-captured audio, 200 SSE only for `wait:true`, name `data.stream_realtime` as
  the preview toggle, and pin a `400 stream_realtime_with_audio_data` error that is never
  emitted.
- **Impact:** a client setting `stream_realtime:true` (preview typing off) gets zero preview
  frames; a fire-and-forget client expecting 202 JSON gets a long-lived SSE stream; clients
  keying on status/content-type mis-handle every call.
- **Fix:** either read `stream_realtime` in `cmd_record` and honor the 202/200-JSON shapes,
  or rewrite the docs to the actual behavior (always 200 SSE; preview via `preview`) and drop
  `stream_realtime` + its error from the contract.
- **Resolved (branch `refactor/audit2-cleanup`, chose "implement the documented contract"):**
  `POST /transcribe` now honors all four cases. `wait:false` → `202
  {message:"Recording started"}` with the recording **detached** from the connection (runs in
  the background, stopped via `POST /transcribe/stop`; write-mode still types the result);
  `wait:true` → `200` SSE ending in `done`, and `stream_realtime:true` additionally streams
  `preview` frames. `stream_realtime` is decoupled from preview-typing at the handler by
  claiming the shared preview slot only when set (`recording/preview.rs` runs the loop when a
  slot is claimed **or** preview-typing is on, and types only when write-mode **and**
  preview-typing are on). The `wait:true` SSE path keeps "client disconnect → stop the
  recording" intact (Phase-2 `manual_stop_tx` signal). Client-side: `TranscribeOptions` gained
  `stream_realtime`; the shared `transcribe()` parses the `202` JSON for `wait:false` (the CLI
  already handled a `None` transcription); the app sends `stream_realtime:true` to keep its
  live-preview panel. Two review-caught minors folded in: aligned the docs' request-body
  example to the actual **flat** top-level option shape (`transcribe.md`/`transport.md` had
  nested them under `data`, which the daemon never read for `write_mode`/`stop_mode`/`preview`),
  and documented the fire-and-forget `202`'s optimistic (no-completion-guarantee) semantics on
  the busy-race. The pre-existing busy-check TOCTOU is left as-is (Tier 3 #6).

### [ ] 9. 🟠 API design: `daemon_status_changed` payload is an untyped hand-matched contract whose keys have already drifted

- **Where:** built inline with `serde_json::json!` string keys
  (`lifecycle.rs:108-124`, `device_management.rs:208-214,310-311`, `switch.rs:115-120`) and
  re-parsed inline with matching `.get("…")` keys in the app
  (`handlers/daemon/events.rs:54-186`).
- **Problem:** it's a rich discriminated union (`loading_model`, `model_switched`, `ready`,
  `switching_device`, `device_switch_error`, `active_backend_changed`, …) with no typed
  representation in any crate (unlike `download_progress`/`language`, which are typed). The
  drift is already observable: `switching_device` emits `to_device` while
  `loading_model_for_device` emits `target_device`, and the app reads each spelling
  separately.
- **Impact:** a field rename or client typo is a silent runtime degradation with no
  compile/test signal — a missing `model_name` drops the whole event; a renamed
  `provider`/`source` falls back to stale state.
- **Fix:** define a `#[serde(tag="status")]` `DaemonStatusEvent` enum in shared with typed
  fields (`Provider`, `Device`); construct/deserialize it on both sides; normalize
  `to_device`/`target_device`; pin the schemas in `docs/protocol/scopes/daemon_status.md`.

### [ ] 10. 🟠 Duplication: `MAX_MANIFEST_BYTES` is triplicated across the indexer, install, and custom-repo paths

- **Where:** `super-stt-indexer/src/assets.rs:22` (`u64`, enforced at publish),
  `registry/install.rs:26` (`u64`, at install-time manifest download),
  `registry/custom_repo.rs:22` (`usize`, at custom-repo resolve) — all 256 KiB, hand-synced,
  gating the same `backend.toml` at different lifecycle points.
- **Problem:** this is the green-publish/failed-install drift class Tier 2 #4 eliminated for
  tarball budgets by moving them into `registry-types::verify` — the manifest cap was left
  behind. Raising the indexer's cap without raising `install.rs` makes a backend publish
  cleanly yet fail every install at 256 KiB.
- **Fix:** hoist a single `MAX_MANIFEST_BYTES` into `registry-types::verify` beside the
  tarball budgets; reference it from all three (casting to `usize` where needed).

### [ ] 11. 🟡 Duplication: install-time manifest verification uses raw `toml::from_str` instead of `Manifest::parse`

- **Where:** `verify_manifest_bytes` does `toml::from_str::<Manifest>(&text)`
  (`registry/install.rs:584`) then re-implements only the entrypoint-safety subset inline
  (`:592-595`).
- **Problem:** `Manifest::parse` (`registry-types/manifest.rs:509-544`) applies the
  destination-traversal check, the `file`-xor-`parts` invariant, and the empty-`file`→`None`
  normalization (the Tier 1 #25 fix) that raw deserialization skips.
- **Impact:** the install path hand-maintains a partial copy of the parser's safety rules; a
  future guard added to `parse` (as UnsafeDestination and the xor-normalization already were)
  silently won't apply at install verification.
- **Fix:** use `Manifest::parse(&text)` and drop the inline re-check, keeping only the
  install-specific index-consistency assertions.

### [ ] 12. 🟡 Protocol/Docs: undocumented endpoint `POST /v1/active_model/reload`

- **Where:** registered at `http/v1/settings/mod.rs:102-105` → `reload_active_model`
  (dispatches `Command::ReloadActiveModel`).
- **Problem:** `docs/protocol/endpoints/v1/active_model/` documents only `cancel.md`;
  no doc anywhere describes this live, mutating, settings-scoped endpoint.
- **Fix:** add a `reload.md` sibling (or a section in `active_model.md`) documenting scope,
  semantics, success response, and the recording/switch conflict errors; or remove the route
  if it isn't part of the external contract.

### [ ] 13. 🟠 Tests/CI: subprocess transport orchestration is never compiled or run anywhere in CI

- **Where:** `super-stt-daemon/tests/subprocess_mock.rs` is gated by
  `#![cfg(all(feature="subprocess-backends", feature="test-fixtures"))]` **and** an
  `SUPER_STT_TEST_SUBPROCESS=1` env early-return (`:44-46`).
- **Problem:** `test-fixtures` is not a default feature, so `cargo test`/CI compiles the file
  to an empty crate; `just check` runs clippy without `--all-targets`, so it never builds the
  integration-test target either. The env gate keeps it from running even if it did.
- **Impact:** one of the daemon's two production transports has zero automated coverage and
  can bit-rot undetected — a refactor breaking `SubprocessBackend` passes CI green, unlike
  the WASM path which CI explicitly builds and runs.
- **Fix:** give the subprocess mock a hermetic CI path (like the WASM mock), or at minimum a
  step that compiles it (`cargo test --features test-fixtures --no-run --test subprocess_mock`
  or `just check --all-targets --all-features`); document that runtime coverage is
  systemd-gated.

---

## Tier 3 — per-crate cleanup & hardening

### Daemon — blocking work & performance

### [ ] 1. 🟠 XDG portal typing backend rebuilds a fresh `zbus::Proxy` on every keysym press/release *(perf)*

- **Where:** `notify_keysym` does `zbus::Proxy::new(…)` per call
  (`output/keyboard/xdg_portal_backend.rs:123-130`); `type_text` calls it 2–4× per char,
  `backspace_n` 2× per deleted char.
- **Impact:** the portal is the first backend auto-detection picks on modern COSMIC/Wayland,
  so this is the default typing path. During the preview loop the daemon re-types the growing
  transcript every ~2s — hundreds of backspaces + retypes per tick for a long dictation, each
  spawning a proxy construct/drop with D-Bus match-rule churn, on the latency-sensitive
  interactive path.
- **Fix:** build the `RemoteDesktop` proxy once in `XdgPortalBackend::new()` (or cache it in a
  field) and reuse it; it's immutable for the session.

### [ ] 2. 🟡 Preview loop resamples audio synchronously on the request's async worker every tick *(blocking)*

- **Where:** `resample_and_emit_preview` calls `utils::audio::resample(...)`
  (`recording/preview.rs:219`) on the async worker before handing the chunk to
  `transcribe_audio_chunk` (which **is** `spawn_blocking`).
- **Problem:** the offload boundary is drawn one call too late; each preview tick resamples up
  to 5s of capture on the worker, and the final drain resamples the whole recording.
- **Fix:** fold the resample into the existing transcription `spawn_blocking`, or
  `spawn_blocking` the resample itself.

### [ ] 3. 🟡 Registry index cache does blocking `std::fs` + JSON (de)serialization inside the async refresh path *(blocking)*

- **Where:** `Client::refresh` is async but `load_from_disk` (`std::fs::read`,
  `registry/client.rs:177`) + `serde_json`, `persist` (`:185-195`), and `from_env`'s
  `create_dir_all` (`:76`) are synchronous.
- **Fix:** move the file IO + serialization behind `spawn_blocking`/`tokio::fs`, keeping only
  the in-memory `RwLock` swap on the async path.

### [ ] 4. 🟡 SSE fan-out serializes every event twice (struct → Value → String) per subscriber *(perf)*

- **Where:** `AnyReceiver::recv_json` does `serde_json::to_value(evt)` (`events.rs:350`); the
  forwarder then `serde_json::to_string`s that `Value` in `format_sse_frame`
  (`transcribe.rs:186`). The intermediate `Value` is built and discarded.
- **Impact:** once per event per subscriber; `frequency_bands` is emitted at the cpal-callback
  rate, so every visualization subscriber pays a throwaway allocation + a second full
  serialization on every frame.
- **Fix:** for the statically-typed topics, serialize the typed event directly to the SSE
  `data:` string and skip the `Value` hop; the three genuinely-heterogeneous topics
  (`daemon_status_changed`, `download_progress`, `registry_install`) keep the current path.

### [ ] 5. 🟡 Audio input callback clones the mono buffer (i16: allocates a throwaway intermediate) on the RT thread every chunk *(perf)*

- **Where:** `process_audio_data_f32_with_streaming` does
  `samples_tx.send(mono_samples.clone())` (`audio/processing.rs:106`) before passing
  `&mono_samples` on; the i16 variant allocates a full interleaved `Vec<f32>`, downmixes into
  a second Vec, then clones that (`:120-125`) — three allocations where one suffices.
- **Impact:** runs inside the cpal input callback on the audio RT thread every buffer;
  allocation there risks buffer overruns / dropped input.
- **Fix:** compute what `process_audio_samples` needs first, then **move** `mono_samples` into
  the send; for i16, downmix directly from the `&[i16]` slice into a single mono Vec.

### [ ] 6. 🟡 `spawn_recorder` installs the manual-stop channel before the authoritative busy claim, so a losing race nulls the winner's stop channel *(concurrency)*

- **Where:** `*self.manual_stop_tx = Some(stop_tx)` at `recording/preview.rs:24`, before
  `setup_recording_session`'s check-and-set of `busy` (`recording/mod.rs:182-188`); the loser's
  cleanup (`:31`) sets `manual_stop_tx = None`.
- **Impact:** narrow today (the `/transcribe` preview-slot claim single-flights starts), but on
  the `/transcribe/stop`→fresh-`record` path a losing racer can null the winner's stop channel,
  leaving it unstoppable (toggle reports "in progress", disconnect stop is a no-op, the 1-minute
  timeout finds `None`) until restart.
- **Fix:** claim `busy` first and install `manual_stop_tx` only after the claim succeeds.

### [ ] 7. 🟡 Session-persist channel is unbounded and accumulates full-map snapshots while a keyring write is stalled *(resources)*

- **Where:** `mpsc::unbounded_channel` (`tokens.rs:76`); `persist_loop` only coalesces after
  `persist_snapshot` returns (`:97-102`), which awaits `spawn_blocking(set_sessions_blob)`
  (`:131`); the cooldown short-circuit is checked only at the start.
- **Problem:** a write stalled on a D-Bus unlock prompt (not yet failed) holds the loop while
  every mint/revoke enqueues a full clone of the sessions map — the stalled-consumer hazard
  Tier 3 #8 bounded for the `/events` SSE channel, left unbounded here.
- **Fix:** bound to a capacity-1 "latest wins" mailbox (older snapshots are strict subsets), or
  refuse to enqueue while a write is in cooldown/in-flight.

### Daemon — security & correctness

### [ ] 8. 🟡 Write-mode types untrusted backend output into the focused window with no control-character sanitization *(security)*

- **Where:** `preprocess_text` only trims/collapses whitespace + capitalizes
  (`output/preview.rs:11-43`); `process_final_text` types the result directly
  (`typer.rs:280-288`).
- **Problem:** backends are explicitly untrusted, yet C0/C1 controls (ESC, BEL, NUL,
  backspace), bidi overrides, and zero-width chars aren't whitespace and survive to the
  simulator. `SECURITY.md:82` claims "Control character filtering" the write path doesn't do
  (newlines *are* neutralized by the whitespace collapse, bounding severity).
- **Fix:** strip/reject control (and optionally bidi/zero-width) chars before the simulator in
  both `update_preview` and `process_final_text`; align the `SECURITY.md` claim with what's
  enforced.

### [ ] 9. 🟠 Consent flow prompts with an unverifiable `<unknown>` binary instead of failing closed *(security)*

- **Where:** `resolve_peer_exe()` falls back to `PathBuf::from("<unknown>")` when the pid is
  absent or the `/proc/<pid>/exe` readlink fails (`auth/consent.rs:223-238`); `request.rs:79`
  uses it verbatim as the consent_key, popup "Executable" line, and the minted token's
  `exe_path`.
- **Problem:** this contradicts the module's own model ("the user verifies a *binary*, not a
  self-reported name") — once the exe is `<unknown>` the only identity left is the
  attacker-controlled `app_name`. readlink fails under exactly the hardened setups the comments
  anticipate (ProtectProc/hidepid, Yama ptrace_scope) or pid recycling.
- **Impact:** on such a host every prompt shows an unverifiable binary; all requests collapse to
  one consent_key (one Deny blocks all, one Allow mints a 30-day token bound to a path matching
  no executable — which the `/events` exe-watch then spuriously revokes).
- **Fix:** treat an unresolved exe as fail-closed (return a deny/`PopupFailed`, or refuse to
  mint); represent it as `Option<PathBuf>`/an explicit enum so callers can't treat the sentinel
  as a real identity.

### [ ] 10. 🟡 `/auth/request` has no rate limit and no popup concurrency cap *(security)*

- **Where:** `ConsentLocks` dedups only on `(exe_path, normalized-scopes)`
  (`consent.rs:42-47`); `/auth/request` is mounted outside every `require_rate_limit` group
  (`v1/mod.rs:91`); each popup is a `KeyboardInteractivity::Exclusive` overlay awaited up to 60s
  (`consent/main.rs:103-108`, `consent.rs:122`).
- **Impact:** the 8 known scopes yield 255 distinct non-empty subsets, all valid distinct keys.
  A same-uid process can fire one request per subset and drive up to 255 concurrent
  exclusive-keyboard overlays — a desktop-lockout / consent-fatigue DoS that weaponizes the
  trusted security UI; the deny-cache is no defense (every subset is a fresh key).
- **Fix:** a global semaphore (capacity 1) around popup spawning so at most one dialog is on
  screen, and/or a per-peer rate limit returning a transient error instead of spawning.

### [ ] 11. 🟡 Consent dialog headlines the attacker-controlled `app_name` with no "unverified" framing *(security)*

- **Where:** `view_window` builds the body as `"{request_label} wants access…"` straight from
  the untrusted `STT_AUTH_APP_NAME` (`consent/main.rs:142-147`); the trusted `exe_path` is a
  plain `Executable: {path}` line (`:157`) with no visual weight.
- **Impact:** a same-uid binary sets `STT_AUTH_APP_NAME` to "Super STT Settings" and the prompt
  reads as first-party; users decide on the spoofable headline, not the raw exe path.
- **Fix:** lead with the verified binary basename and label `app_name` as claimed/unverified,
  or visually subordinate it to the exe path.

### [ ] 12. 🟡 Interpreter/loader exe paths collapse distinct clients to one consent identity *(security)*

- **Where:** identity is `/proc/<pid>/exe` (`consent.rs:229-231`), which for interpreted/wrapped
  clients resolves to the interpreter (`/usr/bin/python3`, an Electron/AppImage runtime).
- **Impact:** two unrelated Python/Electron clients share one consent identity — denying one
  blocks all under that interpreter, and a grant is effectively for the interpreter, with no
  signal that the shown path isn't the requesting program.
- **Fix:** at minimum surface the limitation in the dialog and docs; longer-term augment identity
  (cmdline/argv[0] or an attested client id) rather than exe alone.

### Daemon — API design

### [ ] 13. 🟡 `(String, Provider, String)` model-identity triple is primitive-obsessed — name and source are transposable Strings

- **Where:** threaded through `find_model`/`list_models` (`stt_models/backends/mod.rs:189,222`),
  `pick_startup_model`/`first_local_model` (`discovery.rs:30,53`), `AppModel.available_models`,
  `ModelMessage::AvailableModelsLoaded`, and the `current_model`/`current_provider`/`current_source`
  field triple.
- **Impact:** nothing distinguishes `(model, provider, source)` from `(source, provider, model)`;
  a transposed pair at any of ~7 sites compiles and fails only at runtime.
- **Fix:** introduce `ModelId { name, provider, source }` (or a `Source(String)` newtype) in
  registry-types/shared; construction becomes named-field and transposition a type error.

### [ ] 14. 🟡 Config clear-mutators leave `preferred_provider` (and `update_active_backend` leaves `preferred_source`) stale

- **Where:** `update_preferred_model` sets all three (`config.rs:241-244`), but
  `clear_preferred_model`/`clear_active_backend` clear model+source only (`:250-267`) and
  `update_active_backend` clears only `preferred_model` (`:257-260`) despite its doc saying it
  drops the loaded-model preference.
- **Impact:** `preferred_provider` is write-once-never-cleared; the persisted triple can be
  internally inconsistent (`model="", provider="whisper", source=""`). Latent today
  (`pick_startup_model` guards on `preferred_model.is_empty()` first) but a phantom preference for
  any future consumer that reads provider/source without that guard.
- **Fix:** have both clear paths also reset `preferred_provider`, and `update_active_backend`
  clear the full triple, so the three fields are always set and cleared together.

### App

### [ ] 15. 🟡 Startup double-loads all model/device/backend state and fires failing requests while disconnected

- **Where:** `initial_load_tasks` schedules a standalone `LoadInitialData` after a fixed 500ms
  sleep (`init.rs:69`); since Tier 1 #14, `handle_daemon_connected` also runs it on the
  disconnected→connected transition (`daemon/mod.rs:132-136`), and the startup ping resolves in
  well under 500ms.
- **Impact:** on a healthy start the full batch (list_models, current_model, device,
  active_backend, gpu_info, reload_backends, set_allow_online_models) runs twice; when the daemon
  is down it fires unconditionally (no connection guard), producing failing requests + a spurious
  `ModelError`.
- **Fix:** remove the delayed standalone `LoadInitialData`; rely on the connect-transition load.

### [ ] 16. 🟡 Write-only `last_udp_data` field — an advertised watchdog that does not exist (app copy of applet #23)

- **Where:** `AppModel.last_udp_data: Instant` initialized (`init.rs:106`), written on two SSE
  events (`recording.rs:218,225`), read nowhere.
- **Problem:** the identical write-only "connection health watchdog" field Tier 1 #23 deleted
  from the applet; the app's copy was missed. The doc comment references a UDP path that no
  longer feeds the app (events arrive over SSE).
- **Fix:** delete the field, initializer, and two writes (mirroring the applet #23 fix).

### [ ] 17. 🟡 `PrimaryLanguageSelected` applies optimistically with no rollback on save failure

- **Where:** `handlers/language.rs:33` sets `self.language.primary_language` before the daemon
  call; on `Err` routes to `LanguageError` (banner) without restoring the prior value.
- **Problem:** inconsistent with the confirm-then-apply / rollback pattern Tier 3 #37 established
  for volume/theme/preview-typing/stop-mode/write-method. The success path relies on the
  `settings_changed` SSE, which only fires on a *successful* set.
- **Fix:** capture the prior `primary_language` and restore it in the error handler (as the
  sibling handlers do), or make the set confirm-then-apply.

### [ ] 18. 🟡 Optimistic backend deselect/unload never rolls back; stale "self-heals on next refresh" comment

- **Where:** `DeselectBackend` (`handlers/models_page/mod.rs:330-341`) and `UnloadActiveModel`
  (`:132-147`) optimistically clear state and fire the daemon call as `|_| Action::None`,
  justified by "self-heals on the next refresh".
- **Problem:** the periodic settings refetch that comment assumes was removed by Tier 1 #14;
  `ActiveBackendLoaded` is now fetched only on a reconnect transition. If the daemon rejects the
  clear (mid-recording guard), the UI shows idle while the daemon still holds the backend +
  loaded model (GPU memory) until a full reconnect.
- **Fix:** roll back the optimistic clear on error (like `BackendSelectFailed`/
  `StagedModelLoadFailed` already do); correct the comment.

### Applet

### [ ] 19. 🟡 Applet config enums persist PascalCase (norm violation) and carry a divergent, unused snake_case Display/FromStr

- **Where:** `VisualizationTheme`/`WorkingAnimationTheme`/`VisualizationSide`/`VisualizationColor`
  derive plain serde (persist `"Waveform"`/`"Full"`/`"Blue"`, per
  `fixtures/configs/v0.1.3/applet-full.toml`) while hand-implementing a *different* snake_case
  `Display`/`FromStr` (`"waveform"`, `"b_equalizer"`, …) used only in their own round-trip tests
  (`models/theme.rs:4`).
- **Problem:** this is the wire-enum standardization Tier 2 #2 / Tier 3 #22 applied to the sibling
  `IconAlignment` in the same file (now `icon_alignment = "end"`); these four were left behind, so
  the file is internally inconsistent one field apart, violating the snake_case norm.
- **Fix:** pick one representation — either route persistence through the snake_case ids (`serde(with=…)`
  over the existing FromStr/Display, + a one-time migration so old configs don't reset under
  `deserialize_or_default`), or delete the unused Display/FromStr and keep only `pretty_name`.
  Extend the snake_case-wire-id test to all four enums.

### [ ] 20. 🟡 Pulse visualization ignores `VisualizationSide` — split Left/Right applets render identical pulses

- **Where:** `render_bars` and `WaveformVisualization` call `visible_band_range(side, …)`;
  `PulseVisualization` hard-indexes global `bands[8..24]` and never reads `side`
  (`ui/components/visualizations/pulse.rs:49`). Unlike the Dots animation, it has no documented
  side-split exemption.
- **Impact:** paired Left+Right applets get correctly-split bars/waveform but identical
  full-spectrum pulses — a silent cosmetic inconsistency easy to mistake for a bug.
- **Fix:** honor `side` via `visible_band_range`, or document the exemption the way
  `WorkingAnimationTheme::Dots` is.

### [ ] 21. 🟡 Vestigial UDP naming (`UdpSubscriptionId`, `udp_restart_counter`) now that visualization is SSE-over-unix-socket

- **Where:** `app/mod.rs:38,88`, `app/subscription.rs:21` — with hedging comments ("retained
  while the legacy UDP path is being deprecated").
- **Problem:** visualization is delivered as `frequency_bands` SSE frames over `/events`; there
  is no UDP socket. Tier 3 #23 removed the legacy vestiges but left the naming.
- **Fix:** rename to `EventsSubscriptionId` / `events_restart_counter` and drop the hedge
  comments (mechanical, no behavior change).

### Tests / CI / build

### [ ] 22. 🟡 `just ci` silently skips the daemon's entire WASM transport integration coverage

- **Where:** the `ci` recipe (`fmt-check check check-features test doctest schema-check`,
  `justfile:151`) never runs `build-mock-wasm-backend{,-realtime}`; `just test` is plain
  `cargo test`. With the fixtures unbuilt, `mock_component()` returns `None` and both tests
  early-return "skipping" (`wasm_mock.rs:31-34`) as passes. CI itself builds them (`ci.yml:66-69`).
- **Impact:** a developer running `just ci` gets green without ever exercising the primary
  shipping transport; the local gate diverges from CI.
- **Fix:** have `ci` (or `test`) depend on `build-mock-wasm-backend` + the realtime variant, so
  the tests run instead of self-skipping.

### [ ] 23. 🟡 CI clippy gate does not lint test code (`--all-targets` missing)

- **Where:** `check` runs clippy with no `--all-targets` (`justfile:89`), so integration-test
  targets and `#[cfg(test)]` modules are never linted, and feature-gated test files are neither
  linted nor compiled.
- **Impact:** test code escapes the `-D warnings`/pedantic/`-D unused_must_use` bar; a
  feature-gated or newly-added test can fail to compile without the clippy job noticing.
- **Fix:** add `--all-targets` to `check` (accepting the build cost), or a dedicated
  `cargo clippy --all-features --all-targets` step.

### [ ] 24. 🟡 Topic→scope pin test iterates a hand-maintained list rather than the macro's topic set

- **Where:** `required_scope_matches_shared_mapping` walks a hardcoded 11-entry `[Topic::…]`
  array (`daemon/events.rs:610-622`) — a third copy of the topic list that lives in the `topics!`
  macro (`:207-249`) and the shared mapping (`widget_subscription/mod.rs:139-148`).
- **Impact:** a topic added to the macro but forgotten in the shared `required_scope_for_topic`
  map isn't caught — the "pin" only checks topics named in its own literal, so a client subscribing
  to the new topic gets 403 with no CI failure.
- **Fix:** have `topics!` emit a `Topic::ALL: &[Topic]` and iterate that in the test (and ideally
  in the shared mapping construction) so a missing scope is a compile/CI failure.

### [ ] 25. 🟡 App test-recording client hardcodes the `manual_only` wire string instead of the shared enum

- **Where:** `TranscribeOptions { stop_mode: Some("manual_only".to_string()), … }`
  (`super-stt-app/src/daemon/client/v1/transcribe.rs:48`).
- **Problem:** re-introduces a hand-synced wire token; the shared `wire_enum_strings!` table is
  the source of truth Tier 2 #2 established, and the CLI already drives its values from
  `WIRE_VARIANTS`.
- **Fix:** use `RecordingStopMode::ManualOnly.to_string()`.

### Dependencies & packaging

### [x] 26. 🟠 `just install-app` points at a desktop file that was renamed away — the manual install breaks after copying the binary

- **Where:** `app_desktop_file_src := 'super-stt-app'/'resources'/'app.desktop'` (`justfile:40`),
  used at `:334`. The file is now `super-stt-app/resources/super-stt-app.desktop` (the shell
  installers already reference the correct name).
- **Impact:** `just install-app` installs the binary then aborts at the desktop-entry step with
  `install: cannot stat '…/app.desktop'`, leaving a half-installed app (no desktop entry, no icon).
- **Fix:** update `justfile:40` to `…/'super-stt-app.desktop'` (matching the
  `app_desktop_file_name` constant already at `:39`).
- **Resolved (branch `refactor/audit2-cleanup`):** `app_desktop_file_src` now reuses the
  `app_desktop_file_name` constant (`'super-stt-app'/'resources'/app_desktop_file_name`), so
  it resolves to the real `super-stt-app.desktop` and can't drift from the name again.

### [ ] 27. 🟡 Workspace `tokio` pins `"full"`, so every crate's per-feature narrowing is dead

- **Where:** `[workspace.dependencies] tokio` lists `"full"` alongside
  `macros/net/rt-multi-thread/time` (`Cargo.toml:107-116`); Cargo unions `"full"` over every
  consumer, so the per-crate lists (cli, forge, shared, daemon) are no-ops.
- **Impact:** small clients link the entire tokio surface; the per-crate lists are misleading and
  a drift trap (a crate omitting a feature it needs still builds because `"full"` supplies it).
- **Fix:** pick one convention — drop `"full"` and rely on `default-features=false` + per-crate
  lists, or drop the per-crate lists and document that all crates take full tokio.

### [ ] 28. 🟡 Unused `slab` and `slotmap` direct dependencies in super-stt-app

- **Where:** `super-stt-app/Cargo.toml:29-30` ("Cosmic settings dependencies"), zero references
  in `src`.
- **Fix:** `cargo remove -p super-stt-app slab slotmap` (verify with a build).

### [ ] 29. 🟡 super-stt-shared declares no `license`, unlike every sibling crate

- **Where:** `super-stt-shared/Cargo.toml:1-6` sets no `license`; `[workspace.package]` defines
  none to inherit. Every other crate declares `license = "GPL-3.0-only"`.
- **Impact:** the one crate with ambiguous license metadata on a GPL project — drifts from the
  stated policy; a license scan/SBOM/accidental publish sees it unlicensed.
- **Fix:** add `license = "GPL-3.0-only"` to shared, or promote it into `[workspace.package]` and
  inherit everywhere.

### [ ] 30. 🟡 systemd unit hardcodes stale Arch CUDA env vars that reach nothing after de-candling

- **Where:** `Environment=CUDA_PATH=/opt/cuda` + `LD_LIBRARY_PATH=…`
  (`super-stt-daemon/systemd/super-stt.service:18-19`).
- **Problem:** the daemon is de-candled (GPU residency lives in subprocess backends launched via
  `systemd-run --user` with explicit `--setenv`, which don't inherit the daemon's env), so these
  Arch-specific paths decorate only the daemon process that needs none of it.
- **Fix:** remove the two `Environment=` lines; if GPU backends need a library path it goes through
  the `systemd-run --setenv`/`ReadOnlyPaths` provisioning, documented there.

### [ ] 31. 🟡 `cargo audit` recipe exists but is never run by CI or `just ci`

- **Where:** the `audit` recipe (`justfile:172`) is invoked by no workflow and omitted from
  `just ci`.
- **Impact:** RUSTSEC advisories against the large (libcosmic-heavy, git-sourced) dependency tree
  are never caught automatically.
- **Fix:** add a scheduled/PR CI job running `cargo audit` (e.g. `rustsec/audit-check`), kept out
  of the per-PR fast path if runtime is a concern.

### CLI

### [ ] 32. 🟡 super-stt-cli declares 5 unused dependencies (hyper, hyper-util, http-body-util, serde, serde_json)

- **Where:** `super-stt-cli/Cargo.toml:20-24`; the single-file crate references none of them (all
  network calls go through `super_stt_shared::daemon::http_client`).
- **Impact:** misleading dependency surface — a reader sees hyper + http-body-util and assumes the
  CLI hand-rolls a transport, inviting the reimplementation the shared client prevents.
- **Fix:** `cargo remove -p super-stt-cli hyper hyper-util http-body-util serde serde_json`
  (keep anyhow, clap, tokio, super-stt-shared).

### [ ] 33. 🟡 CLI-requested SCOPES are hardcoded literals with no conformance guard against the shared catalog

- **Where:** `SCOPES: &[&str] = &["transcribe", "status"]` (`super-stt-cli/src/main.rs:27`) — the
  set is correct/least-privilege, but has no link to `scopes::KNOWN_SCOPES`.
- **Problem:** Tier 2 #8 added a conformance test for the consent binary's scope list; the CLI's
  requested-scope list got no equivalent, so a wire-scope rename fails only at runtime.
- **Fix:** add a `#[test]` (or debug_assert) asserting `SCOPES.iter().all(|s| scopes::is_known_scope(s))`.

### i18n

### [ ] 34. 🟡 super-stt-app's i18n-embed/Fluent subsystem is fully vestigial: advertised localizability, zero localized strings

- **Where:** the `fl!` macro (`i18n.rs:43-51`), `FluentLanguageLoader`, `RustEmbed` bundle,
  `i18n.toml`, and `main.rs:12-15` wiring exist, but `fl!(…)` is never invoked outside its own
  definition; every user-facing string is a hardcoded English literal (~161 in `ui/views/` alone);
  none of the six FTL IDs is referenced. Startup still runs locale detection + eagerly parses the
  fallback bundle (with an `.expect(...)` at `i18n.rs:36` that can panic on a malformed embedded
  asset).
- **Impact:** three dead dependencies + an embedded folder as dead weight, per-launch work for
  output that never reaches the UI, a startup panic risk, and a false contract (a contributor
  adding a locale sees zero effect).
- **Fix:** either remove the subsystem (delete `i18n.rs`/`i18n.toml`/`i18n/`, the `main.rs` wiring,
  the three deps) if localization isn't near-term, or make it load-bearing by routing strings
  through `fl!`. Don't leave it half-wired.

### [ ] 35. 🟡 Duplicate FTL bundle: `super_stt_app.ftl` (underscore) is a dead orphan never read by the loader

- **Where:** `i18n/en/` holds two byte-identical bundles; the loader derives its domain from
  `CARGO_PKG_NAME` = `super-stt-app`, so only `super-stt-app.ftl` (hyphen) loads. The underscore
  copy is embedded (whole-folder `#[folder="i18n/"]`) but never resolved.
- **Impact:** two files to keep in sync, one authoritative — editing the underscore copy has no
  effect.
- **Fix:** delete `i18n/en/super_stt_app.ftl` (or both, if #34 removes the subsystem).

### Install scripts

### [ ] 36. 🟡 Shell installers leak their `mktemp` working dir (tarball + extracted binaries) on every run

- **Where:** `install-stable.sh:27` creates `TEMP_DIR` but its cleanup trap is commented out
  (`:679`); `install-beta.sh:51` has no trap.
- **Impact:** each install/update leaves a `/tmp/tmp.XXXX` holding the multi-hundred-MB tarball +
  binaries until reboot.
- **Fix:** add `trap 'rm -rf "$TEMP_DIR"' EXIT` after `TEMP_DIR` is set in both.

### [ ] 37. 🟡 `uninstall.sh` does not remove the PATH export the installers append to the shell rc

- **Where:** `update_path` appends `export PATH="$HOME/.local/bin:$PATH"` to `.bashrc`/`.zshrc`
  (`install-stable.sh:360-365`, `install-beta.sh:381-386`); `uninstall.sh` never touches the rc.
- **Impact:** install/uninstall asymmetry — a leftover `export PATH` line survives a full
  uninstall, unmentioned by the "What is PRESERVED" notice.
- **Fix:** strip the exact appended line on uninstall, or list it under the preserved-state notice.

---

## Rejected candidates (surfaced but did not survive verification)

Recorded so they aren't re-litigated:

- **Recording-state-machine busy TOCTOU (claimed high):** the exploitable double-capture is
  unreachable — the `/transcribe` handler's preview-slot claim single-flights record starts before
  `handle_command` is ever called, so a losing racer never reaches `spawn_recorder`. The residual
  smell (busy-claim not self-atomic) is captured narrowly as Tier 3 #6.
- **`keyring_unavailable` pseudo-error-code in `message`:** the secret endpoints deliberately serve
  `503 keyring_unavailable` as a documented REST contract string; no consumer switches on
  `error_code` there. Not the drift class it resembles.
- **`active_backend` persists a dir-basename vs `source`:** the basename is deterministically
  derived from `source` and documented as the deliberately-stable handle; the two fields are
  semantically distinct (selected backend vs loaded-model source), not one value stored twice.
- **Audio-theme audition failure rolls back a persisted theme:** the daemon's
  `handle_test_audio_theme` returns `success` even on beep-playback failure, so an
  audio-device-busy audition does not trigger the rollback; only a rare transport error in the
  window between the two calls would, which is a generic two-step edge.
- **Installers drop icons in the flat dir:** `~/.local/share/icons` **is** an XDG icon base
  directory that GTK/COSMIC search via the unthemed fallback; the app icon name-matches its desktop
  entry and resolves. `uninstall.sh` treats the flat layout as supported.
- **Default install routes to the deprecated stable layout:** the `install.sh` header, the beta
  script, and the release workflow all document this as an intentional, temporary rollout state
  (new layout ships under `--beta` until promoted) — an in-repo-documented trade-off, not a defect.

---

## Suggested order of attack

1. **Install path (Tier 1 #1, #10, Tier 2 #2, Tier 3 #26)** — the flagship shortcut is broken on
   `just install`; low-risk, high user-visible payoff, and unblocks a working manual install.
2. **Auth durability + bypass hardening (Tier 1 #4, #5, #6)** — the `SessionPersister` ordering
   and shutdown-drain regressions plus the release env bypasses are the security-relevant cluster;
   all three are small, localized changes.
3. **Docs truthfulness (Tier 2 #1–5)** — rewrite `SECURITY.md` and the README install/CLI/online
   sections to the current model; pure documentation, no code risk, retires theme #2.
4. **Error-code completion (Tier 2 #6, #7, #9, #10)** — finish migrating producers onto the
   `error_code` envelope the prior audit built (dead `OnlineModelsDisabled`/`InvalidDevice`
   variants, registry/backend envelopes, the untyped status event).
5. **Blocking/perf batch (Tier 1 #3, Tier 3 #1–5)** — the record-start cpal offload and the
   per-keysym proxy rebuild are the two with plausible user-visible latency; the rest are cheap
   consistency wins.
6. **Per-crate cleanup (remaining Tier 3)** — dead deps, dead i18n, applet naming, optimistic
   rollback gaps, CI gate widening.

---

## Strengths to preserve

- The first audit's fixes all held: the `error_code` layer, `wire_enum_strings!`, the
  registry-types canonical parser + `verify` budgets, the async `Simulator` and `SessionPersister`
  scaffolding, and the scope/logging/paths consolidations are the right foundations — most Tier 2
  findings here are "finish applying the pattern you already built", not "the pattern is wrong".
- The `spawn_blocking` + `handle_get_gpu_info` idiom, the epoch-guarded model-load path, and the
  confirm-then-apply settings handlers are the correct templates for the blocking-work and
  optimistic-rollback findings above.
- The COSMIC panel-integration, the sandboxed backend model, and the consent trust-gate are sound
  in design; the consent findings here are hardening (fail-closed, rate-limit, framing), not a
  rearchitecture.
