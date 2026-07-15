# Code Quality Audit — July 2026

Full-workspace review covering all nine crates (~54k lines of Rust). Conducted after
the three audit-cleanup passes (#267, #269, #270), so lint-level issues and trivial
dead code were excluded by design; findings below are structural, behavioral, or
contract-level. File:line references are to `main` as of 2026-07-07. Checkboxes track
resolution; resolved entries carry a **Resolved** note with the PR.
Severity: 🔴 high · 🟠 moderate · 🟡 minor.

---

## Systemic themes

Three patterns account for the majority of findings:

1. **Contracts maintained by hand in N places.** The index.json schema, wire-string
   enums, HTTP status mapping, the consent scope list, and tar/sha256 validation each
   have 2–4 hand-synced copies — and nearly every pair has already drifted in an
   observable way.
2. **Error identity lives in prose.** `DaemonResponse` carries only free-form
   `message` strings. Status codes are derived by substring matching (already broken —
   Tier 1 #1), guard messages are re-worded at each call site, and `HttpError`
   classification happens by string comparison at three sites. A machine-readable
   error-code layer retires this entire drift class.
3. **Blocking work on the async runtime.** Typing, beeps, keyring DBus calls, and
   config saves all stall tokio workers; the XDG portal backend builds a fresh
   thread + tokio runtime per keysym.

---

## Tier 1 — genuine defects worth fixing

### [x] 1. 🟠 Daemon: state-conflict responses return 500 instead of 409 (substring status mapping has drifted)

- **Where:** `status_code_for_response`
  (`super-stt-daemon/src/daemon/http/internal/helpers/dispatch.rs:52-106`) maps
  response text to status codes by substring matching.
- **Problem:** the phrase list has drifted from the live wire strings:
  - `switch_guard` messages ("Cannot change the backend during active recording…",
    "…during active real-time transcription sessions.",
    `daemon/model_management/switch.rs:33-41`) match no conflict phrase.
  - "Cannot reload the model during active recording…" (`lifecycle.rs:38-44`)
    matches nothing.
  - "Recording already in progress. Please wait…" (`daemon/recording/mod.rs:44-47`)
    matches nothing.
  - Two `CONFLICT_PHRASES` are dead: "Cannot switch devices when" and
    "recording in progress" (`dispatch.rs:86-87`) match no wire string in the crate.
- **Impact:** `POST/DELETE /v1/active_backend`, `DELETE /v1/active_model`, model
  reload, and the `POST /v1/transcribe` race path all return 500 for ordinary state
  conflicts that should be 409.
- **Fix:** add the missing phrases as a stopgap; the real fix is machine-readable
  error codes (Tier 2 #3), which retires the substring mapping entirely.
- **Resolved (`3d7aaea`, branch `refactor/daemon-audit-batch`):** added the missing conflict
  prefixes (`Cannot change the backend during`, `Cannot reload the model during`,
  `Recording already in progress`), dropped four dead phrases, and pinned the guard
  strings with a characterization test. Also aligned `DELETE /active_model`'s doc from
  400 to 409. Stopgap over the substring matcher; Tier 2 #3 remains the real fix.

### [x] 2. 🔴 Daemon: realtime sessions are never removed or cancelled

- **Where:** `RealTimeTranscriptionManager`
  (`super-stt-daemon/src/services/transcription.rs:202-412`).
- **Problem:** there is no removal/stop API; `cancellation_token` is never
  cancelled; send failures on a dead broadcast channel are swallowed. The throttling
  fields (`last_emit`, `model_min_interval`) are written but never read. Resampling
  also runs under the sessions write lock (`transcription.rs:307-321`).
- **Impact:** an abandoned session re-runs inference on the same ~15 s tail every
  200 ms indefinitely, and because `switch.rs:37` / `lifecycle.rs:42` gate on
  `get_active_sessions().is_empty()`, one dead session permanently blocks model
  switching until daemon restart.
- **Fix:** remove and cancel sessions when the client goes away (dead-channel send
  is the signal); cancel the token on stop; move resampling out of the write lock;
  delete or wire up the throttling fields.
- **Resolved (`c4fc6d4`, branch `refactor/daemon-audit-batch`):** removed the entire
  `RealTimeTranscriptionManager` instead of hardening it. It was a documented
  placeholder that dropped its result receiver, undocumented, unused by any shipping
  client, and superseded by `GET /v1/transcribe/realtime` (which bridges to the
  backend's `realtime_session()` and gates switching via the model read lock). Also
  dropped the `start_realtime`/`realtime_audio` command variants and the now-vacuous
  `get_active_sessions()` guard branches; the guards keep their recording checks.

### [x] 3. 🟠 Daemon: `handle_unload_active_model` never persists the cleared preference

- **Where:** `lifecycle.rs:184-189`.
- **Problem:** it clears `preferred_model`/`preferred_source` in memory "so a daemon
  restart stays idle" but never saves the config.
- **Impact:** a restart reloads the model the user explicitly unloaded.
- **Fix:** persist after clearing (the `set_config_field` helper in Tier 3 #7 would
  prevent this class of miss).
- **Resolved (`b72f38e`, branch `refactor/daemon-audit-batch`):** replaced the manual
  in-memory clear with a bundled `clear_preferred_model()` config mutator that clears
  and saves together (mirroring `clear_active_backend`), so the on-disk state can't
  drift from memory. Added a round-trip test for the restart-idle invariant.

### [x] 4. 🔴 Daemon: the recording timeout only stops the preview loop, not the recording

- **Where:** `run_preview_loop` breaks after 1 minute
  (`daemon/recording/preview.rs:114-117`), but `collect_and_clear_preview` then
  awaits the recorder indefinitely.
- **Problem:** with speech detected, continuous background noise, and `SilenceOnly`
  stop mode (manual stop refused, `core.rs:113-130`), nothing ever stops the
  recorder.
- **Impact:** capture is unbounded with `busy=true` and a frozen preview.
- **Fix:** send on `manual_stop_tx` on timeout.
- **Resolved (`33d2cdf`, branch `refactor/daemon-audit-batch`):** on the 1-minute guard,
  `run_preview_loop` now signals the recorder's stop channel (the same one the
  manual-stop shortcut uses) before breaking, so the recorder ends cleanly and
  `collect_and_clear_preview` returns the audio captured so far.

### [x] 5. 🔴 Daemon: `DELETE /v1/backends/{source}` skips the mutation guard, strands the loaded model, and invents a third error envelope

- **Where:** `http/v1/settings/backends.rs:14-87`.
- **Problem:** no busy/switch guard; never calls `unload_current_model()`; returns
  `{"error": …}` — neither the house `{"status":"error"…}` shape nor the registry
  envelope.
- **Impact:** GPU memory stays held after uninstall; `GET /status` and
  `GET /active_backend` disagree; clients can't parse the error.
- **Fix:** route through the shared mutation guard, unload the current model first,
  and use the house envelope (folds into Tier 2 #3).
- **Resolved (`599a422`, branch `refactor/daemon-audit-batch`):** the handler now reuses
  `switch_guard()` (→ 409 `backend_busy`), calls `handle_clear_active_backend()` on
  the was-active path (unloads the model + clears active-backend/preferred-model
  before removing the files), and routes errors through the registry envelope helpers
  (matching `POST /registry/install`). Documented the 409/500 modes in the protocol
  doc. Chose the registry envelope over the house one since uninstall is install's
  inverse and returns a registry type.

### [x] 6. 🔴 Daemon: STT failures are masked as success

- **Where:** one-shot transcribe (`daemon/transcription.rs:156-161`), the recording
  flow (`daemon/recording/mod.rs:122-138`), and the SSE terminal event
  (`http/v1/transcribe.rs:170-174`).
- **Problem:** one-shot transcribe maps backend failure to empty-string success; the
  recording flow returns `Ok("[STT error: …]")`.
- **Impact:** write mode types the error text into the user's focused window, and
  HTTP delivers it as a success `done` SSE event despite the contract defining an
  `error` event.
- **Fix:** propagate the failure; emit the contract's `error` SSE event; never type
  error text.
- **Resolved (`b4e67b2`, branch `refactor/audit-batch-hardening`):** `run_transcription`
  propagates `DispatchError::Failed` as an error (no-speech is an `Ok("")` from the
  backend, so `Failed` is a genuine failure). `record_and_transcribe` returns
  `Result<Result<String, String>>` — outer = setup failure, inner `Err` =
  post-capture failure — so `handle_record_internal` surfaces it as an error
  response (HTTP emits the contract's `error` event, not `done`) and it is never
  typed into the user's window. Updated the characterization test.

### [x] 7. 🟠 Daemon: `POST /v1/transcribe` busy-check TOCTOU on the preview slot

- **Where:** busy check at `transcribe.rs:203`; busy is set later in
  `recording/mod.rs:167-175`; the loser nulls the daemon-global preview sender at
  `transcribe.rs:294`.
- **Problem:** two near-simultaneous requests can both pass the busy check; the
  loser unconditionally nulls the shared preview sender.
- **Impact:** the winner's preview stream is silently killed.
- **Fix:** claim the slot under the same write that sets `busy`, and clear only if
  it is still your sender.
- **Resolved (`f511874`, branch `refactor/audit-batch-hardening`):** the shared preview
  slot is now `PreviewSlot = Option<(u64, Sender)>`. A request claims it only when
  free (bailing with a `recording_in_progress` error frame if another holds it) and
  clears it only when the id is still its own, so a losing racer can neither clobber
  nor null the winner's sender.

### [x] 8. 🟠 Daemon: `--no-default-features --features wasm-backends` does not compile

- **Where:** the subprocess stub is missing `&self`
  (`daemon/model_management/instantiate.rs:159-169`), and
  `http/v1/registry/install.rs:32,35` references variants that don't exist in that
  feature set. Verified via `cargo check`.
- **Problem/Impact:** the feature combination is broken and neither combination is
  exercised by CI, so it can't stay fixed.
- **Fix:** repair the stub and variant references; add the feature matrix to CI.
- **Resolved (branch `refactor/audit-daemon-robustness`):** the empirical breakage
  (the `install.rs` variant references had already been de-gated since the audit)
  was: the `#[cfg(not(subprocess-backends))]` `instantiate_subprocess` stub lacked
  `&self`; `daemon_main.rs` called `stt_models::subprocess::cleanup_orphan_units()`
  unconditionally; and `transcribe.rs`'s `SuperSTTDaemon`/`Response` imports plus
  `instantiate.rs`'s `log::warn` import went unused when a transport was off. Added
  `&self`, gated the orphan-sweep call and the wasm-only imports, and fully-qualified
  the one `warn` call. New `just check-features` compiles all three reduced combos
  (subprocess-only, wasm-only, no-backends) under `RUSTFLAGS=-D warnings`, wired into
  `just ci` and the CI clippy job so the drift can't return.

### [x] 9. 🟠 Daemon: unsolicited `304` panics the registry client

- **Where:** `refresh()` does `self.state.read().as_ref().unwrap()` on 304
  (`registry/client.rs:147-152`); `Client::new` also unwraps the builder
  (`client.rs:55-63`).
- **Impact:** a misbehaving proxy can panic the daemon.
- **Fix:** treat an unsolicited 304 with no cached state as a refetch/error;
  propagate builder failure instead of unwrapping.
- **Resolved (branch `refactor/audit-daemon-robustness`):** the 304 branch now takes
  a single write lock and, if the in-memory cache is empty (an unsolicited 304 when
  we sent no `If-None-Match`), returns `ClientError::Unavailable` instead of
  unwrapping `None`. `get()` already degrades that to the disk cache or `Unavailable`,
  so a misbehaving proxy can no longer panic the daemon. Added a mockito test. The
  builder-unwrap sub-point was superseded by the Tier 2 #4 forge consolidation: the
  `reqwest` client is now built once in `super_stt_forge::http::{short,download}_client`
  with a single documented `.expect()` on a path that can't fail for these settings
  (rustls-no-provider does no eager TLS init) — threading `Result` through a dozen
  infallible call sites for an unreachable branch was not worth it.

### [x] 10. 🔴 Daemon: HTTP vs WS egress allowlists diverge in the wasm host *(security)*

- **Where:** `send_request` matches the authority as written
  (`stt_models/wasm/host.rs:67-69`) while `check_host_allowed` matches synthesized
  `host:port` (`host.rs:121-124`).
- **Problem:** `"api.example.com:443"` matches wss but not https, despite
  `host.rs:106-107` documenting identical rules. The hook also does synchronous DNS
  on the runtime and is check-then-connect (TOCTOU, acknowledged at
  `host.rs:84-85`).
- **Fix:** route `send_request` through `check_host_allowed`.
- **Resolved (branch `refactor/audit-daemon-robustness`):** the HTTP hook's inline
  allowlist match (authority-as-written) is gone; `send_request` now derives
  `host` + scheme-default `port` and calls `check_host_allowed`, the exact function
  the `ws` host uses — so `["h:443"]` behaves identically for `https://h/` and
  `wss://h/`. A no-host authority-form request is rejected outright. Added a
  regression test at the shared `check_host_allowed` for the default-port `host:port`
  match. The synchronous-DNS / check-then-connect TOCTOU is unchanged and stays noted
  in the code (a follow-up; not the divergence this item is about).

### [x] 11. 🟠 Daemon: duplicate adaptive-VAD in `RealTimeSession::add_audio_chunk` reintroduces the NaN hazard fixed in #268

- **Where:** `services/transcription.rs:90-132`.
- **Problem:** RMS computes 0/0 = NaN on an empty chunk (the guard exists only in
  `audio/state.rs:78-81`); `recent_levels`/`silence_start`/votes feed write-only
  state that is never read.
- **Fix:** delete the block or reuse `RecordingState`.
- **Resolved (subsumed by Tier 1 #2, PR #275):** the entire `services/transcription.rs`
  file — `RealTimeSession`, its `add_audio_chunk`, the duplicate adaptive-VAD block,
  and the `recent_levels`/`silence_start`/vote fields — was deleted when the
  vestigial `RealTimeTranscriptionManager` was removed (commit `caaff4d`). The
  canonical realtime path is `GET /v1/transcribe/realtime`. Nothing left to fix;
  `RealTimeSession` and `add_audio_chunk` no longer exist in the tree.

### [x] 12. 🟠 Daemon: preview typer accounting mixes bytes and chars

- **Where:** `apply_simple_diff` returns bytes (`output/typer.rs:200,231`) that
  `apply_text_update` adds to char counts (`typer.rs:313-350`).
- **Problem:** deletions are never subtracted; the mismatch warn
  (`typer.rs:361-373`) fires spuriously on non-ASCII and both branches do the same
  thing. Also an unconditional "Failed to type" debug line (`typer.rs:199`) and an
  `i32`/`-1` sentinel API (`find_tail_match_in_text`, `output/preview.rs:72`) with
  O(n²) `Vec<char>` rebuilds.
- **Fix:** count one unit consistently end-to-end; subtract deletions; return
  `Option<usize>` instead of the sentinel.
- **Resolved (branch `refactor/audit-app-error-surface`):** `apply_simple_diff` now
  returns the net **char** delta (chars typed minus chars deleted) instead of a byte
  length; the byte-vs-char reconciliation in `apply_text_update` — whose two branches
  were identical and whose only effect was a spurious non-ASCII warn — was deleted, so
  `actually_typed` is set once to the display text. The misleading unconditional
  "Failed to type" debug now logs the real `type_text` error. `find_tail_match_in_text`
  returns `Option<usize>` (byte offset) and compares char slices in place instead of
  allocating a `Vec<char>` per position; both call sites and the tests use the
  `Option` form (added a rightmost-match case).

### [x] 13. 🟠 App: a single failed `set_volume`/theme POST flips the app to the full-screen connection-error page

- **Where:** volume/theme failures map to `Message::DaemonError`
  (`handlers/recording.rs:100-124`), and `view.rs:140-145` forces the Connection
  page whenever not `Connected`.
- **Impact:** one trivial request failure hijacks the whole UI.
- **Fix:** scope the error instead of escalating to connection state (the shared
  error surface, Tier 3 #11).
- **Resolved (branch `refactor/audit-app-error-surface`):** built the shared
  scope-tagged error slot the "one error surface" item calls for —
  `AppModel::action_error: Option<ActionError>` with an `ErrorScope`
  (`Customization` / `ConfigureBackend`) and an `action_error_for(scope)` accessor.
  Volume/theme/feedback failures now raise `SettingActionFailed { Customization, .. }`
  (rendered as an inline `error_banner` on the Customization page) instead of
  `DaemonError`, so `daemon_status` no longer flips and the connection page is not
  forced. The slot clears on retry, on reconnect, and on sheet open/close.

### [x] 14. 🟡 App: the app refetches all settings every 5 seconds forever

- **Where:** successful pings map to `Message::DaemonConnected`
  (`handlers/daemon/mod.rs:79-84`), whose handler unconditionally runs six settings
  GETs + language load (`daemon/mod.rs:131-142, 243-302`).
- **Problem:** settings-save successes reuse the same ack, so each toggle also
  triggers a full refetch.
- **Fix:** introduce a dedicated `SettingSaved` ack and load settings only on the
  disconnected→connected transition (SSE `settings_changed` already covers
  cross-client sync).
- **Resolved (branch `refactor/audit-app-error-surface`):** `handle_daemon_connected`
  now runs the settings/model/language loads only on the disconnected→connected
  transition; a periodic keep-alive ping (which also resolves to `DaemonConnected`)
  returns `Task::none()`, so the six GETs no longer fire every tick or clobber
  optimistic edits. The two audio save handlers stopped reusing `DaemonConnected` as
  their success ack (they now resolve to `Action::None`), removing the per-toggle
  refetch without needing a separate `SettingSaved` message.

### [x] 15. 🟠 App: failed secret/option saves are invisible

- **Where:** all backend secret/option save failures route to log-only
  `BackendsError` (`handlers/backend.rs:103-211`); `InstallFailedToStart` is
  likewise invisible (`handlers/models_page/install.rs:69-73`); optimistic values
  are never rolled back (`handlers/settings.rs:46,62-65`).
- **Impact:** a failed API-key save shows nothing in the Configure sheet — the user
  believes the key is stored.
- **Fix:** surface the failures and roll back optimistic state (Tier 3 #11).
- **Resolved (branch `refactor/audit-app-error-surface`):** the four backend
  secret/option save handlers now raise `SettingActionFailed { ConfigureBackend, .. }`
  (rendered as an `error_banner` inside the Configure sheet) instead of the log-only
  `BackendsError`, which is kept for catalog-load failures. `InstallFailedToStart`
  records into a new `RegistryState::install_errors` map (mirroring `uninstall_errors`)
  so the Browse card shows "Failed to start: …" and keeps the Install button for a
  retry, rather than silently dropping the entry. The optimistic-state gap is closed
  by making the preview-typing / recording-stop-mode / write-method handlers
  confirm-then-apply: the local value is set only on the daemon ack, so a failed save
  leaves the control on its old, correct value (and the `PreviewTypingError` handler
  no longer abuses the transcription box). Uses the shared error slot from #13.

### [x] 16. 🟡 App: `$HOME` sanitization corrupts messages when HOME is unset

- **Where:** `err.replace(&std::env::var("HOME").unwrap_or_default(), "$HOME")`
  (`handlers/model.rs:120-124`).
- **Problem:** an empty pattern inserts `$HOME` at every char boundary.
- **Fix:** skip the replacement when the variable is missing or empty.
- **Resolved (branch `refactor/audit-app-tier1-16-19`):** extracted a pure
  `sanitize_home(err, home)` that returns the message unchanged when `home` is empty
  (only folding + capping to 200 chars when it is set) and unit-tested the
  empty-HOME regression, the fold, and the cap.

### [x] 17. 🟡 App: `handle_daemon_events` returns from inside the loop

- **Where:** `handlers/daemon/events.rs:16-34`.
- **Problem:** drops all events after the first that yields a task. Latent today
  (producers wrap singletons) but the API takes a `Vec`.
- **Fix:** collect tasks across the whole batch and return them together.
- **Resolved (branch `refactor/audit-app-tier1-16-19`):** the loop now pushes each
  event's task into a `Vec`, processes the whole batch (so `last_event_timestamp`
  advances past every event), appends the final `update_title()`, and returns
  `Task::batch(tasks)`.

### [x] 18. 🟡 App: reconnect force-navigates to Customization

- **Where:** `handlers/daemon/mod.rs:117-129`, contradicting the documented Models
  launch page (`core/app/init.rs:18-24`).
- **Impact:** yanks mid-flow users to another page on every daemon restart.
- **Fix:** don't navigate on reconnect.
- **Resolved (branch `refactor/audit-app-tier1-16-19`):** deleted the
  reconnect-time nav-activate block (and the now-unused `Page` import); the launch
  page stays Models (set in `init.rs`) and the user's current page is untouched
  across a daemon restart.

### [x] 19. 🟡 App: volume slider fires one POST per drag tick

- **Where:** `ui/views/customization.rs:56`, `handlers/recording.rs:120-126`.
- **Fix:** commit on `on_release` or debounce.
- **Resolved (branch `refactor/audit-app-tier1-16-19`):** the slider's drag callback
  (`VolumeChanged`) now only updates the local value; a new `VolumeCommit` wired to
  the slider's `on_release` fires the single `set_volume` POST. A whole drag is one
  request instead of hundreds.

### [x] 20. 🟡 App: language client encodes `source` but interpolates `model` raw

- **Where:** `daemon/client/v1/settings/language.rs:67,89,116`.
- **Impact:** a model name containing `/` produces a malformed path.
- **Fix:** percent-encode both path segments.
- **Resolved (branch `refactor/audit-tier1-20-23`):** all three
  `/backends/{source}/models/{model}/language` builders now `enc(&model)` as well as
  `enc(&source)`, so a `/` (or any reserved char) in a model name yields a valid path.

### [x] 21. 🟠 Applet: empty `frequency_bands` panics/degenerates the bar renderers

- **Where:** `equalizer.rs:60` / `centered_bars.rs:59`.
- **Problem:** `b64_to_f32_vec` degrades malformed input to an empty vec, then
  `bars_to_show - 1` underflows a usize (debug panic; ~1.8e19 spacing in release).
  Same class as the #268 fix.
- **Fix:** guard `bands_to_show == 0` and use `saturating_sub`.
- **Resolved (branch `refactor/audit-tier1-20-23`):** both renderers now
  early-return when `bars_to_show == 0` (which also avoids the `width / 0.0`
  degeneration) and use `saturating_sub(1)` for the centering width. `waveform.rs`
  was checked and is safe — its `bands_to_show - 1` sits inside the
  `0..bands_to_show` loop, never reached at zero.

### [x] 22. 🟡 Applet: `launch_app`/`open_github` never reap children

- **Where:** `app/update.rs:348-389`.
- **Problem:** zombies accumulate for the session-long applet; `launch_app` also
  probes hardcoded `./target/{debug,release}/` dev paths relative to the panel
  process CWD.
- **Fix:** reap spawned children (or detach properly); drop the dev-path probing.
- **Resolved (branch `refactor/audit-tier1-20-23`):** added a `spawn_detached`
  helper that reaps each child in a detached thread, and routed both `open_github`
  and `launch_app` through it. The two `./target/{debug,release}` dev paths (and the
  redundant `which` probe — `Command::new("super-stt-app")` already searches `PATH`)
  are gone; `launch_app` now tries PATH, `/usr/local/bin`, `/usr/bin`.

### [x] 23. 🟡 Applet: the "connection health watchdog" doesn't exist

- **Where:** `last_udp_data` is written four times, read nowhere (`app/mod.rs:45`,
  `update.rs:47,57,229-231,256`).
- **Problem:** the comments advertise a safety net that was removed.
- **Fix:** implement the watchdog or delete the field and comments.
- **Resolved (branch `refactor/audit-tier1-20-23`):** deleted the write-only
  `last_udp_data` field, its initializer, the four writes, and the "connection health
  watchdog" comment (plus the now-unused `Instant` import in `init.rs`). The
  self-healing event subscription is the real reconnection path; there is no separate
  watchdog to advertise.

### [x] 24. 🟠 Shared: `GET /audio_themes` violates the documented wire form

- **Where:** `AudioTheme` derives serde with no `rename_all` (`models/theme.rs:7-31`)
  and is put on the wire via `available_audio_themes` (`protocol/response.rs:49`).
- **Problem:** returns `["Classic","SciFi",…]` where
  `docs/protocol/endpoints/v1/audio_themes.md` pins lowercase. Unnoticed because
  the smoke test only asserts `is_array()`.
- **Fix:** part of the wire-enum normalization (Tier 2 #2); strengthen the smoke
  test to pin the values.
- **Resolved (`refactor/daemon-audit-batch`, folded into Tier 2 #2):** `AudioTheme`
  now serializes via the `wire_enum_strings!` table (snake tokens, `scifi`), so
  `available_audio_themes` returns the documented lowercase form; the smoke test
  now pins the exact value list.

### [x] 25. 🟡 Registry-types: `SubprocessAsset` with `file = ""` plus valid `parts` passes `Manifest::parse`

- **Where:** the XOR guard treats empty as absent
  (`super-stt-registry-types/src/manifest.rs:528-534`), but `release_files()`
  returns `[""]` and `is_multipart()` is false (`manifest.rs:202-213`).
- **Fix:** normalize empty→`None` in parse.
- **Resolved (branch `refactor/audit-tier1-25-29`):** `Manifest::parse` now takes
  `&mut m` and normalizes an empty `file` string to `None` before the XOR check, so
  `file = ""` + valid `parts` yields `file: None` / `is_multipart(): true` /
  `release_files(): [parts…]` instead of a `[""]` filename. Added a regression test.

### [x] 26. 🟡 Shared: `cmd_record` silently drops an invalid `stop_mode`

- **Where:** `.ok()` at `protocol/dispatch.rs:123-128`, while
  `set_recording_stop_mode` 400s the same input (`dispatch.rs:237-248`); the
  `disable_silence_detection` compat branch (`dispatch.rs:129-142`) is undocumented
  and caller-less.
- **Fix:** one unknown-value policy across both paths (house rule:
  fallback-to-default); delete the compat branch.
- **Resolved (branch `refactor/audit-tier1-25-29`):** both paths now **reject** a
  present-but-unknown value with an error and change nothing —
  `cmd_set_recording_stop_mode` keeps its documented `400 invalid_recording_stop_mode`
  (leaving the stored setting untouched), and `cmd_record` (made fallible) rejects an
  invalid override instead of silently dropping it to `None`. This is the single
  unknown-value policy the item asked for; per-request state-changing writes validate
  strictly rather than silently coercing to the default. The dead
  `disable_silence_detection` compat branch and its test are gone; new tests pin the
  reject-on-invalid behavior on both paths.

### [x] 27. 🔴 Forge: `ForgeClient::download` buffers unbounded bodies *(security)*

- **Where:** `github.rs:143-147` reads the whole body into memory;
  `custom_repo.rs` size-checks only declared (attacker-controlled) metadata before
  download and applies the real cap only after.
- **Fix:** add `max_bytes` to the trait and stream with an accumulating check — the
  pattern already exists at `registry/install.rs:633-650` and
  `indexer/src/assets.rs:144-171`.
- **Resolved (branch `refactor/audit-tier1-25-29`):** `ForgeClient::download` gained a
  `max_bytes` param; the GitHub adapter now streams `resp.chunk()` with a running
  total and aborts with the new `ForgeError::TooLarge` the instant the body would
  exceed the cap (no full buffering). The one production caller (`custom_repo.rs`
  manifest fetch) passes `MAX_MANIFEST_BYTES` and maps `TooLarge` →
  `ManifestTooLarge`, retiring the after-the-fact `len()` check. Added a
  cap-rejection test.

### [x] 28. 🟠 Indexer: one malformed `repo` string aborts the entire index build

- **Where:** `RepoRef::parse(&entry.repo)?` (`indexer/src/main.rs:96`).
- **Problem:** bypasses the carry-forward resilience path every other per-entry
  failure uses.
- **Fix:** carry the entry forward like the rest.
- **Resolved (branch `refactor/audit-tier1-25-29`):** a `RepoRef::parse` error is now
  turned into a `BuildFailure` and routed through the same per-entry carry-forward
  `match` as every other failure, instead of `?`-propagating out of the build loop.

### [x] 29. 🟡 Indexer: multi-GB temp parts leak on mid-loop errors

- **Where:** `indexer/src/main.rs:246-262` — `?` returns before the cleanup loop.
- **Fix:** use a `TempDir`/`Drop` guard.
- **Resolved (branch `refactor/audit-tier1-25-29`):** a `TempParts` RAII guard owns the
  downloaded part paths (registered *before* each download) and removes them all on
  drop, so an early `?` from `resolve_url` / `download_to_file` / validation no longer
  leaks the parts already fetched. The explicit post-loop cleanup is gone.

### [x] 30. 🔴 Consent: the auto-approve env path ships in release builds *(security)*

- **Where:** `STT_AUTH_AUTO_APPROVE_AFTER_MS` (`consent/src/main.rs:288-318`).
- **Impact:** makes the human trust gate self-approve.
- **Fix:** gate behind `#[cfg(debug_assertions)]` or a test feature.
- **Resolved (branch `refactor/audit-tier1-30-31`):** `maybe_spawn_auto_approve_timer`
  is now `#[cfg(debug_assertions)]`; release builds get a `#[cfg(not(...))]` no-op
  stub, so the env-var bypass of the human trust gate is compiled out of shipped
  binaries entirely. `cargo test` builds with `debug_assertions` on, so the
  `http_smoke_full` integration test (its only user) still works.

### [x] 31. 🟠 Daemon: "update" uses string equality, so it happily downgrades

- **Where:** `registry/update.rs:167`, where the app's check is strictly-newer
  semver (`app/.../installed.rs:42-49`); the indexer has two more subtly different
  `v`-prefix strippers (`resolve.rs:96-98`, `manifest.rs:45`).
- **Fix:** one shared `parse_version`/`update_available`.
- **Resolved (branch `refactor/audit-tier1-30-31`):** added
  `super_stt_registry_types::version` with `parse_version` (single-`v`-prefix strip +
  semver) and `update_available` (strictly-newer, `false` on any non-semver). The
  daemon update handler no-ops unless the registry is strictly newer (no more
  downgrades or reformatted-string false updates); the app's `installed.rs` check,
  the indexer's `resolve::parse_semver`, and its `manifest::validate` all delegate to
  it. Dropped the app's now-redundant local `update_available` + tests (moved to the
  shared crate) and its now-unused `semver` dep.

---

## Tier 2 — cross-crate standardization targets

Ranked by drift risk. These answer "what should be standardized or reused."

### [x] 1. 🔴 index.json schema + manifest→index mapping duplicated across three crates

- **Where:** nine structs exist twice — producer (`indexer/src/index_json.rs:14-125`,
  Serialize) and consumer (`daemon/src/registry/index_schema.rs:48-195`,
  Deserialize) — sync'd by comment only, with `IndexStale` a third time
  (`shared/src/registry/mod.rs:86`).
- **Problem:** confirmed drift: secret labels fall back to the secret name (indexer
  `main.rs:326`) vs `""` (daemon `custom_repo.rs:145,154`); option `type` defaults
  `"string"` vs `""` (`custom_repo.rs:155`); option `default` is `expect()` vs
  silently dropped; `license` required vs `#[serde(default)]`. The same backend
  renders differently depending on install path. The manifest→`IndexBackend`
  synthesis (`into_index_backend`, `classify_models`, id-from-source) is likewise
  tripled across `custom_repo.rs:175-209` / `local_dir.rs:59-117` /
  `indexer/main.rs:273-351` — and `local_dir` silently drops `secrets`/`options`
  (`local_dir.rs:107-108`) where `custom_repo` maps them.
- **Fix:** use the proven layering — canonical types in `super-stt-registry-types`,
  daemon keeps `check_min_client`/`retain_safe_backends` as extensions (like
  `validate_runtime` for `Manifest`). Fold the field-identical
  `RegistryModel`/`RegistrySecret`/`RegistryOption`
  (`shared/src/registry/mod.rs:41-91`) into the same leaf types. Move the
  manifest→`IndexBackend` synthesis to one shared implementation.
- **Resolved (`7b27bed` + `7dfbfad`, branch `refactor/daemon-audit-batch`):**
  Phase 1 (`7b27bed`) moved the nine structs + `SCHEMA_VERSION`/`MIN_CLIENT` into
  `super-stt-registry-types::index` (Serialize + Deserialize derived together);
  `license` is lenient on read and always written;
  `RegistryModel`/`RegistrySecret`/`RegistryOption`/`IndexStale` are aliases of the
  canonical leaves; the daemon's min-client soft-floor and unsafe-path filter stay
  daemon-side as free functions over `Index`. Phase 2 (`7dfbfad`) moved the
  manifest→`IndexBackend` synthesis (`from_manifest` + `id_from_source` +
  `model_support`) into that same module and pointed all three install paths at it,
  fixing the local-dir secrets/options/license drop and the custom-repo label/type
  drift (name-fallback labels, `"string"` option-type default).

### [x] 2. 🟠 One wire-string enum convention

- **Where:** `RecordingStopMode`, `WriteMethod`, and `AudioTheme` each carry 2–3
  live string forms — PascalCase serde persisted in daemon.toml via
  `config.rs:53-100`, kebab-case `Display` on the response wire, aliased `FromStr`
  on input.
- **Problem:** three unknown-value policies (silent default / hard error / hard
  error) and undocumented aliases (`silence|both|manual`, `xdg|portal|wayland`)
  that violate the no-legacy-aliases rule.
- **Fix:** one macro generating serde/Display/FromStr from a single table —
  snake_case, fallback-to-default, no aliases — with `deserialize_or_default`
  covering config migration. Requires a protocol-docs pass first (docs currently
  pin kebab-case; decide kebab vs snake deliberately). Includes fixing Tier 1 #24.
- **Resolved (`refactor/daemon-audit-batch`):** chose **snake_case** (the house
  rule). New `wire_enum_strings!` macro generates `Display`/`FromStr`/serde from
  one table for all three enums — one `snake_case` token each
  (`silence_and_manual`, `xdg_desktop_portal`, `scifi`), `FromStr`/`Deserialize`
  reject unknown (REST 400), config resilience stays with `deserialize_or_default`.
  Dropped every alias (`silence`/`both`/`manual`, `xdg`/`portal`/`wayland`) and the
  kebab forms. Protocol docs rewritten to snake first. **Wire-breaking**: clients
  sending the old kebab tokens now get the default, and pre-snake daemon.toml
  values (PascalCase) migrate to default on load (accepted trade-off; the config
  still loads without a full reset). Folds in Tier 1 #24.

### [x] 3. 🔴 Machine-readable error codes on `DaemonResponse`

- **Where/Problem:** error identity today lives in prose, so every consumer matches
  strings.
- **Fix — retires:** the substring status mapping (Tier 1 #1); the four re-worded
  switch guards (`switch.rs:31-43`, `switch.rs:259-282`, `lifecycle.rs:37-45`,
  `device_management.rs:157-180` → one `guard_model_mutation(action)`); the
  `HttpError` string classification (`shared/daemon/session.rs:236`,
  `widget_subscription/mod.rs:188-193`, `app/core/app/events.rs:20` — root cause:
  `with_token`'s `op` erases to `Result<T, String>`; have it return
  `HttpResult<T>`); and the ad-hoc error envelopes (Tier 1 #5).
- **Resolved (`5d31afc` + `103792b` + `c123f27`, branch `refactor/daemon-audit-batch`):**
  Phase 1 (`5d31afc`) landed the mechanism — a typed `ErrorCode` enum in shared
  (snake_case wire, `#[serde(other)] Unknown` for forward-compat, `http_status()` as
  the single code→status source of truth), an `error_code` field +
  `error_with_code()` constructor on `DaemonResponse`, and `status_code_for_response`
  deriving the status from `error_code`; `transport.md` documents the contract.
  Phase 2 (`103792b`) unified the four guards into `guard_model_mutation(action)`,
  migrated the daemon's 400/409 producers to carry codes, and deleted the drifted
  substring matcher (`status_code_for_response` is now pure `error_code → status`).
  Phase 3 (`c123f27`) typed the client error path end-to-end: `with_token`'s `op`
  returns `HttpResult<T>`, so the retry and the `is_user_denied` /
  `classify_daemon_error` decisions match the typed `HttpError` variant instead of
  its wording (`is_wire_invalid_session` deleted). The ad-hoc uninstall envelope was
  folded in under Tier 1 #5.

### [x] 4. 🟠 Download/verify plumbing

- **sha256:** `verify_sha256` at `stt_models/download.rs:64,185` compares
  case-insensitively; `registry/install.rs:432,499,602` compares `==`. One helper.
- **Tar entry-safety + budgets:** identical escape checks duplicated
  (`install.rs:522-532` vs `indexer/assets.rs:197-213`), but the daemon's per-entry
  4 GiB cap and zip-bomb budget (`install.rs:27,536-556`) are missing at publish
  time — a green publish can fail every install. Extract predicate + budgets into
  registry-types and run install policy in the indexer.
- **HTTP client factory:** five builders, four styles — forge (`github.rs:73-77`,
  expect), registry client (`client.rs:58-62`, unwrap, no UA), model download
  (`download.rs:220-223`), pipeline (`pipeline.rs:113-123`, unwrap_or_default), and
  the indexer's bare `Client::new()` (`indexer/main.rs:85`) — **no timeout at all**.
  One `short_client()`/`download_client()` with a workspace UA.
- **Atomic writes:** three tmp+rename variants in the daemon
  (`install.rs:344-359`, `registry/client.rs:170,198`, `download.rs:191`); the
  indexer writes `index.json` non-atomically and its two modes disagree on a
  trailing newline (`main.rs:131-133` vs `local.rs:67-70`). One `write_atomic`
  helper.
- **Chunk loop:** within `install.rs`, `stream_download` (`:366-416`) and
  `download_verified_parts` (`:443-511`) duplicate the loop;
  `stt_models/download.rs:101-193` is a third implementation with better
  conventions (tmp+rename, cancellation, sync_all). Extract
  `stream_to_file(http, url, cap, dest, on_chunk) -> sha256`.
- **Resolved (`refactor/audit-batch-hardening`), all 5 sub-parts:**
  a new `super-stt-registry-types::verify` hosts the shared download-verify
  policy — `sha256_matches` (case-insensitive; fixed three case-sensitive `==`
  compares in `install.rs`), the tar entry-safety predicate, and the unpack
  budgets. The indexer now enforces those budgets **at publish**
  (`validate_subprocess_parts`), so a zip-bomb that would fail every install is
  rejected up front (regression test added). A `super-stt-forge::http` factory
  (`short_client`/`download_client`, workspace UA) replaces the five ad-hoc
  builders and fixes the indexer's timeout-less `Client::new()`. A
  `super-stt-registry-types::fs::write_atomic` (tmp + fsync + rename) replaces
  the daemon cache write and the indexer's two non-atomic `index.json` writers
  (which also disagreed on a trailing newline — now consistent). Finally,
  `download_stream::stream_body_to_writer` unifies the three chunk loops
  (single-file, multi-part append, cancellable model download): a writer-based
  helper that hashes, enforces an optional cap, honors an optional cancellation
  predicate, and reports per-chunk progress, with the caller owning file
  lifecycle / verify / error mapping.

### [x] 5. 🟠 Daemon connection supervisor + retry policy

- **Where:** three reconnect policies against the same daemon — shared
  `next_backoff` (doubling 1s→30s, no jitter), applet `RetryStrategy`
  (`applet/src/daemon/retry.rs:12-60`, exponential + jitter — the best), app flat
  5 s sleep (`handlers/daemon/mod.rs:158-166`).
- **Problem:** the applet's subscription bridge, `RetryAuthorization` handling,
  ping cycle, and even the scope-coverage test are a hand-maintained fork of the
  app's (`applet subscription.rs:20-63`, `update.rs:116-165,280-346` vs
  `app subscription.rs:11-101`, `handlers/daemon/mod.rs:74-208`).
- **Fix:** promote `RetryStrategy` and a generic subscription bridge into shared;
  per-app topics/scopes/messages stay per-crate.
- **Resolved (`refactor/audit-batch-hardening`):** the retry *policy* is unified across
  all three clients. `RetryStrategy` (exponential + ±10% jitter) now lives in
  `super-stt-shared::daemon::retry`; the applet uses it directly (its local copy +
  test deleted), the shared widget-subscription reconnect loop drives it instead
  of the jitter-less `next_backoff` doubling (so the SSE reconnect jitters too),
  and the settings app reconnect drops its flat 5 s sleep for a
  `reconnect_retry: RetryStrategy` field on `AppModel` (advanced on failure, reset
  on connect). Also fixed a latent `% 0` panic in `next_delay` for sub-10 ms
  initial delays. The generic subscription bridge (`run_widget_subscription`) was
  already shared; both clients' remaining `subscription.rs` are thin per-crate
  adapters (config + update→message mapping), which is by design — per the fix's
  "per-app topics/scopes/messages stay per-crate".

### [x] 6. 🟡 Logging init

- **Where:** five variants — byte-identical RUST_LOG-else-Info blocks in
  `app/main.rs:9-17` and `consent/main.rs:341-347`, parameterized in
  `daemon_main.rs:18-33`, one-liner in `indexer/main.rs:64`, bare
  `env_logger::init()` in the applet (defaults to **Error** — effectively silent vs
  siblings), and nothing at all in the CLI.
- **Fix:** one `shared::logging::init(default_level)`. Also: the daemon loads
  config *before* initializing logging (`daemon_main.rs:93` vs `:114`), so the
  "config invalid, reset to defaults" warning (`config.rs:173`) is silently
  dropped — init logging first.
- **Resolved (branch `refactor/audit-tier2-6-7-8`):** added
  `super_stt_shared::logging::{init, init_with}` (RUST_LOG wins, else the given
  default). Wired the app, CLI (previously none), applet (was silent — now `Info`
  like its siblings), consent, and daemon; the daemon now inits logging **before**
  `DaemonConfig::load()`, so the config-invalid warning is captured. `env_logger`
  moved out of four crates (now only shared + the standalone indexer). The indexer
  is deliberately left on its own one-liner rather than coupling a build tool to the
  daemon-protocol crate for a logging call.

### [x] 7. 🟡 XDG path helpers

- **Where:** `get_config_path` is byte-identical daemon↔applet
  (`daemon/config.rs:154-163` vs `applet/config/settings.rs:67-76`); three
  incompatible `dirs`-miss fallbacks coexist (HOME-else-/tmp, `env::temp_dir()`,
  raw `XDG_RUNTIME_DIR`); the subprocess socket path
  (`stt_models/subprocess/mod.rs:103-106`) rebuilds `$XDG_RUNTIME_DIR/stt/…` while
  bypassing the validated `secure_socket_path` helper
  (`shared/validation/paths.rs:29-61`).
- **Fix:** one `shared::paths` module; route the subprocess socket through the
  validated helper.
- **Resolved (branch `refactor/audit-tier2-6-7-8`):** added
  `super_stt_shared::paths::{config_dir, data_dir, cache_dir}` (each keeping its
  call sites' existing fallback, so no behavior change) and routed all five
  production dir sites through it — the daemon+applet `get_config_path`, the
  backends `data_dir`, and the registry-client/pipeline `cache_dir`. The private
  socket validator was generalized to `pub fn secure_runtime_path(relative)`; the
  subprocess socket now goes through it (`backends/<name>.sock`) instead of a raw
  `$XDG_RUNTIME_DIR` join, so it inherits the traversal/prefix/length guards. Dropped
  the applet's now-unused `dirs` dep.

### [x] 8. 🟡 Smaller shared items

- `accept_base_url` duplicated verbatim (`forge/lib.rs:139-147` vs
  `daemon/registry/mod.rs:17-22`) — keep forge's, import it.
- CryptoProvider install ×3 (+1 redundant call at `download.rs:219`); one
  `install_crypto_provider()` beside the client factory.
- Consent scope list hand-mirrors the daemon's `KNOWN_SCOPES`
  (`consent/main.rs:200-212` vs `responses.rs:96-105`); a new scope renders the
  "deny is safe" warning on legitimate prompts. Shared leaf data + conformance
  test.
- CLI hard-codes stop-mode strings (`cli/main.rs:68`); derive `clap::ValueEnum` on
  the shared enum.
- SSE framing loop duplicated in the shared client
  (`http_client/v1/transcribe.rs:131-162` vs `events.rs:82-122`) — generic
  `sse::block_stream`; also fix the stale NDJSON doc comments in transcribe.rs.
- Hand-rolled `unsafe` pin projection (`http_client/internal/transport.rs:17-42`)
  re-implements `http_body_util::Either` — the crate's only `unsafe`, deletable.
- **Resolved (branch `refactor/audit-tier2-6-7-8`):** all six:
  `accept_base_url` is now `pub` in forge (tests moved there); the daemon re-exports
  it. `install_crypto_provider` lives in `forge::http` beside the client factory
  (forge `rustls` promoted from dev-dep); the daemon re-exports it, the redundant
  `download.rs` call is gone, and the indexer's inline install is dropped (its unused
  `rustls` removed). `KNOWN_SCOPES`/`is_known_scope` moved to
  `super_stt_shared::daemon::scopes`; the daemon re-exports, consent gained a
  conformance test that every known scope has a specific description (no
  "deny is safe" fall-through). The CLI's `stop-mode` values come from the shared
  `RecordingStopMode::WIRE_VARIANTS` (added to `wire_enum_strings!`). The two SSE
  loops collapse into `sse::block_stream(body, parse_block, on_error)`; the stale
  "NDJSON" docs now say SSE. `RequestBody` is now
  `http_body_util::Either<Empty, Full>`, deleting the hand-rolled `unsafe` pin
  projection.

---

## Tier 3 — per-crate cleanup backlogs

Indexer/forge/consent/cli defects live in Tier 1 #27–#31; their shared plumbing is
Tier 2 #4/#6/#8.

### [x] 1. 🟡 Daemon: wasm/subprocess backends duplicate the `/v1` client

- **Where:** ~35 lines already drifted — `wasm/mod.rs:400-443` vs
  `subprocess/mod.rs:324-361`, plus `invoke` vs `request`, `status`/`ping` vs
  `wait_for_ping`.
- **Fix:** extract `build_transcribe_body` / `parse_transcribe_response` / a small
  `V1Transport` trait.
- **Resolved (branch `refactor/audit-tier3-1-4`):** added a feature-agnostic
  `stt_models::v1` module with `build_transcribe_body` and
  `parse_transcribe_response` (the byte-identical, drifting parts); both backends'
  `transcribe_audio` now call them. Kept the transports (`request` over a Unix
  socket vs `invoke` through the WASM component) per-backend — they are genuinely
  different, not duplication — so a `V1Transport` trait bought nothing over the two
  free functions. Added unit tests for the shared build/parse.

### [x] 2. 🟠 Daemon: device-switch success/recovery duplicate ~120 lines and bypass graceful unload

- **Where:** `device_management.rs:236-321,361-421` vs `switch.rs:229-257`.
- **Problem:** both bypass `unload_current_model()`'s graceful shutdown, dropping
  the model under the write lock.
- **Fix:** one `finalize_loaded_model()`; route unload through the real path.
- **Resolved (branch `refactor/audit-tier3-1-4`):** added
  `finalize_loaded_model()` (normalize device → record actual device → install the
  `LoadedModel`, returning the device label); the model-switch finalize and both
  device-switch finalize sites (success + recovery) now call it instead of
  re-implementing the normalize/model-set/actual-device triple. `prepare_device_switch`
  no longer drops the model under the write lock — it routes through
  `unload_current_model()`, so the backend is `shutdown()` outside the lock like every
  other unload path. `unload_current_model`/`finalize_loaded_model` are
  `pub(in crate::daemon)` so the sibling device-switch module can reach them.

### [x] 3. 🟠 Daemon: config persistence has three idioms

- **Where:** self-saving `update_*` (blocking `fs::write` under the tokio config
  write lock, errors swallowed), pure mutation + `persist_config()`, and
  both-at-once (double/triple writes: `switch.rs:243-249`,
  `device_management.rs:113-118,288-296`, `theme_handlers.rs:30-39,74-83`) — plus
  neither (Tier 1 #3).
- **Fix:** make all mutators pure, persist via `persist_config()` in
  `spawn_blocking`; `save()`'s `Box<dyn Error>` → `anyhow::Result`.
- **Resolved (branch `refactor/audit-tier3-1-4`):** all eight `update_*`/`clear_*`
  config mutators are now pure (no `self.save()`); `persist_config_static` snapshots
  the config under the lock, releases it, and does the blocking TOML-serialize +
  `fs::write` in `spawn_blocking` — so no `fs::write` runs on an async worker under a
  config lock. The six former double-write sites become single-write automatically;
  the five sites that relied on the self-save (`update_active_backend` ×2,
  `clear_active_backend`, `clear_preferred_model`, `update_backend_option`) gained an
  explicit `persist_config()`, including the pre-load `active_backend` write that must
  survive a load failure across a restart. `save()` now returns `anyhow::Result`.

### [x] 4. 🟠 Daemon: blocking work on the async runtime

- **Where:** the portal backend spawns a thread + new tokio runtime per keysym then
  joins (`xdg_portal_backend.rs:122-153`, two runtime `expect`s); enigo sleeps per
  chunk; `play_beep_sequence` spin-waits whole beep durations and
  `handle_test_audio_theme` calls it inline (`theme_handlers.rs:96-161`); keyring
  DBus runs inline in auth middleware and secret endpoints (`tokens.rs:198-230`,
  `secrets.rs:34`, `backend_config_handlers.rs:145,162`).
- **Fix:** make `Simulator` async (portal already holds an async zbus connection)
  or `spawn_blocking` throughout; `handle_get_gpu_info`
  (`device_management.rs:460-468`) shows the right pattern.
- **Resolved (branch `refactor/audit-tier3-1-4`):** the two hot paths that actually
  stall an async worker are now off-runtime via `spawn_blocking`.
  - **Beep** — added `play_beep_sequence_async` (the `spawn_blocking` form of the
    spin-waiting `play_beep_sequence`); `handle_test_audio_theme` and the recording
    start-sound (`recorder::play_start_sound_and_wait`, now `async`) await it instead
    of blocking the worker for the sound's full duration.
  - **Keyring** — added `{get,set,delete,has}_backend_secret_async` (`spawn_blocking`
    wrappers); the async secret handlers (`handle_set_backend_secret`,
    `handle_clear_backend_secret`, the `secrets.rs` list/get endpoints) and the
    WASM model-load secret read (`instantiate::backend_headers`) now await these, so a
    locked-keyring DBus stall no longer parks a runtime thread.
- **Deferred (follow-up, tracked as Tier 3 #35):** the keyboard `Simulator` path
  (portal thread-per-keysym, enigo per-chunk sleeps) and the session-store keyring
  writes (`TokenStore::flush_snapshot`/`load_persisted`). The `Simulator` is a sync
  state machine driven while a `!Send` `std::Mutex` guard (`actually_typed`) is held
  across the call, so offloading it needs the larger async-`Simulator` rewrite the
  fix lists as the primary option (the portal path already spawns its own OS thread,
  so it does not park a runtime worker today — only wastes a runtime per keysym).
  `flush_snapshot` is called from the sync `mint`/`revoke`; a naive detached
  `spawn_blocking` there would race the persist ordering, and `load_persisted` is a
  one-time startup cost — both want a dedicated single-writer persist task rather than
  a point wrap.

### [x] 5. 🟡 Daemon: mutex poison recovery copy-pasted ~10× in `audio/`

- **Where:** `recorder.rs`, `processing.rs`, `device.rs`, while the rest of the
  crate uses parking_lot.
- **Fix:** switch audio to `parking_lot::Mutex` (cpal callbacks are sync-safe with
  it) or one `lock_recover` helper.
- **Resolved (branch `refactor/audit-tier3-5-8`):** switched the three audio
  mutexes (`audio_buffer`, `recording_state`, `audio_device_cache`) from
  `std::sync::Mutex` to `parking_lot::Mutex`. `parking_lot` guards carry no poison
  state, so all 11 `match .lock() { Ok => .., Err(poisoned) => poisoned.into_inner() }`
  recovery blocks collapse to a direct `.lock()` — the cpal real-time callbacks stay
  sync-safe. The type leaks through `get_audio_buffer_ref` into the preview loop, so
  `RecordingSession::preview_buffer` and its one lock site moved too; the sibling
  `actually_typed` stays `std::sync::Mutex` (not shared with a callback). Dropped the
  now-false `# Panics` (poison) doc on `check_output_device_health`.

### [x] 6. 🟡 Daemon: inflight cleanup hand-rolled in 8+ places despite an RAII guard

- **Where:** `install_inflight.write().remove()` on every error path
  (`install.rs:149-241`, `update.rs:70-168`) while `pipeline.rs`'s `InflightGuard`
  is only used inside the spawned task.
- **Fix:** construct the guard at insert.
- **Resolved (branch `refactor/audit-tier3-5-8`):** added `InflightMarker` in
  `pipeline.rs` — a lightweight RAII guard that inserts the source (atomic
  check+insert under one write lock) and removes it on `Drop`, but emits **no**
  event (the synchronous phases fail with plain HTTP errors, not `Failed` install
  events — unlike the pipeline's event-emitting `InflightGuard`). Phase 2 of both
  handlers now returns the marker; the fallible phases (and the update no-op early
  return) just `return`, so the marker's `Drop` cleans up. The happy path calls
  `marker.defuse()` after spawning, handing removal duty to the pipeline's
  `InflightGuard`. This deletes all 11 hand-rolled `install_inflight.write().remove()`
  calls; the now-unused `source_key`/`source`/`&AppState` params drop from the phase
  helpers.

### [x] 7. 🟡 Daemon: settings handlers repeat a 25-line mutate→persist→respond block 6×

- **Where:** `settings_handlers.rs:12-250`.
- **Fix:** one `set_config_field` helper (also prevents Tier 1 #3 recurrences).
- **Resolved (branch `refactor/audit-tier3-5-8`):** added `set_config_field`
  (lock → mutate closure → `persist_config`, returning the persist `Result`) and a
  `settings_saved` response helper (folds the persist outcome into the response,
  appending `(save failed: {e})` and logging a warning on failure while keeping the
  in-memory change). The four simple setters — preview typing, recording stop mode,
  write method, custom models dir — now route through both, dropping their
  hand-rolled `{ lock; mutate }` + `persist_config().await` + Ok/Err match.
  `handle_set_allow_online_models` keeps its explicit mutate/persist: it interleaves
  an async online→local revert between the mutate and the persist and builds a
  revert-aware message, so it doesn't fit the couple-mutate-and-persist shape.

### [x] 8. 🟠 Daemon: SSE fan-out uses unbounded channels

- **Where:** `http/v1/events.rs:75-76,164-190`.
- **Problem:** a stalled reader buffers `frequency_bands` frames without bound.
- **Fix:** bounded channel; drop visualization frames on overflow.
- **Resolved (branch `refactor/audit-tier3-5-8`):** the per-connection `/events`
  channel is now `mpsc::channel(SSE_CHANNEL_CAPACITY = 256)` (was
  `unbounded_channel`), read out via `ReceiverStream`. A shared `try_emit_sse_event`
  helper does `try_send`: a full channel drops the frame (the reader is stalled —
  shed it, logging a warn) and only a `Closed` channel tears the forwarder down.
  Keepalive uses the same `try_send` (drop the heartbeat when full; cancel only on
  `Closed`); the `subscribed` ack and `revoked` frame go through it too. `frequency_bands`
  is the dominant volume, so those are what overflow drops in practice; the
  `/transcribe` stream keeps its unbounded channel (bounded per-recording lifetime,
  non-droppable `preview`/`done`/`error` frames). Frame formatting is now a shared
  `format_sse_frame` used by both the bounded fan-out and the unbounded
  `emit_sse_event`.

### [x] 9. 🟡 Daemon: minor items

- Five identical `emit_*` DBus wrappers (`services/dbus.rs:131-198`).
- Dead spinner scaffolding in `transcribe_with_spinner`
  (`recording/transcribe.rs:64-121`).
- `PipeExt` trait for one `.pipe(Ok)` (`recorder.rs:580-593`).
- Keyring sessions-blob accessors bypass `kv_get`/`kv_set` with a second mock
  mechanism (`keyring.rs:157-247`).
- Stringly `Result<_, String>` in keyring/download-progress.
- **Resolved (branch `refactor/audit-tier3-9-11`):**
  - The five `emit_*` DBus wrappers collapse to an `emit_signal!` macro (they
    differed only in the signal method + event type; a generic async helper can't
    express it — the closure would return a future borrowing the emitter). The
    repeated object path is now an `OBJECT_PATH` const, reused by `.at(..)` too.
  - `transcribe_with_spinner` → `transcribe_final`: the spinner apparatus was
    entirely dead (`spinner_handle` never assigned, cancel/counter never read), and
    the `_typer`/`_write_mode` params were unused — all removed.
  - Deleted the one-use `PipeExt` trait; the single `.pipe(Ok)` is now `Ok(..)`.
- **Deferred (follow-up, Tier 3 #36):** the keyring sessions-blob → `kv_get`/`kv_set`
  unification and the `Result<_, String>` → typed-error conversion. The first changes
  session-persistence behavior under `SUPER_STT_KEYRING_MOCK` (the sessions blob would
  move from the keyring-crate mock to the process-global `mock_store`), which the
  `http_smoke_full` restart test exercises — wants its own verified change. The second
  is a type-system change rippling through every keyring/download-progress caller
  (including the async wrappers added in Tier 3 #4) for marginal benefit; better as a
  focused pass than bundled here.

### [x] 10. 🟡 App: group the flat ~90-variant `Message` enum into sub-enums

- **Where:** routing is declared twice — nine `matches!` lists in
  `core/app/update.rs:21-211` + per-handler `_ => Task::none()` catch-alls.
- **Problem:** a forgotten variant silently no-ops.
- **Fix:** sub-enums make dispatch exhaustive and delete both lists.
- **Resolved (branch `refactor/audit-tier3-9-11`):** the flat 103-variant `Message`
  is now 12 per-area sub-enums (`ShellMessage`, `DaemonMessage`, `ModelMessage`,
  `ModelsPageMessage`, `DeviceMessage`, `DownloadMessage`, `PreviewTypingMessage`,
  `RecordingStopModeMessage`, `WriteMethodMessage`, `BackendMessage`, `LanguageMessage`,
  `RecordingMessage`) wrapped by `Message`, plus the still-top-level `SettingActionFailed`
  (handled inline). `dispatch` is one exhaustive `match` (the twelve `matches!` lists are
  gone), and each `handle_*_messages` takes and `match`es its own sub-enum — so all the
  top-level `_ => Task::none()` catch-alls are deleted and a forgotten variant is a compile
  error at both ends. `From<XMessage> for Message` impls exist for ergonomics. Now that the
  group name carries the context, the three redundant-prefix settings enums shed it
  (`PreviewTypingMessage::Toggled`, `RecordingStopModeMessage::Changed`, etc.). The few
  intra-group delegate sub-handlers (daemon, models_page, download) keep a narrow
  `_ => Task::none()` since they each receive the full sub-enum but handle a subset — the
  exhaustiveness guarantee lives at the sub-enum boundary, which is what silently no-op'd
  before.

### [x] 11. 🟠 App: one error surface

- **Where:** four ad-hoc patterns today — transcription-box hijack, log-only,
  invisible, escalate-to-connection-page (Tier 1 #13/#15).
- **Fix:** the `ModelError` → in-card banner path (`handlers/model.rs:118-131`,
  `ui/views/models/active.rs:437-445`) is the good template; add a shared
  scope-tagged error slot rendered per page, and roll back optimistic state on
  failure.
- **Resolved (branch `refactor/audit-tier3-9-11`):** generalized the existing
  scope-tagged `action_error` slot. `ErrorScope` gained `Recording` and
  `InputSimulation`; the Recording and Input Simulation pages now render the shared
  `error_banner` (via `action_error_for(scope)`) like Customization already did. Added
  `set_action_error`/`clear_action_error` helpers (scope-safe: clearing one page's
  banner can't wipe another's). Converged the ad-hoc error paths onto it:
  - **Log-only → banner:** the preview-typing / stop-mode / write-method save errors
    and the language-save error now populate their page's banner instead of only
    `log::warn!`.
  - **Transcription-box hijack → Models card:** `DeviceError` and `DownloadError` set
    `ModelOperationState::Error` (the good template) instead of overwriting the
    Recording page's `transcription_text`.
- **Deferred (follow-up, Tier 3 #37):** rolling back optimistic state on failure
  (audio theme / volume / select-backend / staged-device). Doing it right needs a
  captured previous value threaded through a per-setting success/failure message
  (the failure arrives as a separate message that doesn't carry the prior value), and
  the drift self-heals on the next reconnect refetch (`VolumeLoaded` /
  `CurrentAudioThemeLoaded` / `ActiveBackendLoaded`), so it's a refinement rather than
  a correctness gap. The `escalate-to-connection-page` behavior (whole-UI takeover on
  `DaemonStatus::Error`) is intentional for a lost daemon and stays.

### [x] 12. 🟡 App: `clear_loaded_model()` helper for the copy-pasted current-model triple

- **Where:** the triple assignment (`current_model`/`current_provider`/
  `current_source`) was copy-pasted at seven sites, each hand-picking adjacent
  resets (`handlers/models_page/mod.rs:144-149, 281-284, 299-302`,
  `handlers/model.rs:128-130`, `handlers/download.rs:115-127`,
  `core/app/small_state.rs:95-97`).
- **Resolved (PR #274):** the triple now lives in one `clear_loaded_model()` helper
  in `small_state.rs`; the call sites delegate to it.

### [x] 13. 🟡 App: shared task builders

- **Where:** registry catalog fetch, `list_backends` reload, and ping are re-rolled
  at 3–4 sites each.
- **Fix:** `fetch_registry_catalog()`, `reload_backends()`, `ping_task()` beside
  the existing `build_load_settings_tasks()`.
- **Resolved (branch `refactor/audit-tier3-13-17`):** moved `build_load_settings_tasks`
  into a new `handlers/tasks.rs` and added `ping_task()`, `reload_backends()`, and
  `fetch_registry_catalog(refresh: bool)` beside it. The 3 ping sites (+ startup), 3
  `list_backends` reloads, and 3 registry-catalog fetches now call the shared builders
  (the one `refresh`-then-`list` site passes `refresh: true`).

### [x] 14. 🟡 App: type the language payload

- **Where:** `model_language: Option<serde_json::Value>` parsed field-by-field in
  two views (`core/app/mod.rs:162-168`, `ui/views/models/active.rs:100-125`,
  `ui/views/language_picker.rs:25-35`).
- **Fix:** deserialize a `LanguageResolution` struct at the client boundary.
- **Resolved (branch `refactor/audit-tier3-13-17`):** added a `LanguageResolution`
  struct (`state`) deserialized in the `get_/set_/clear_model_language` client fns; the
  field, the `ModelLanguageLoaded` payload, and the two views now use typed
  `effective`/`source`/`primary`/`supported` fields instead of `.get("...")` on a
  `serde_json::Value`.

### [x] 15. 🟡 App: split `AppModel` (~45 fields)

- **Fix:** extract `ModelsPageState` and `LanguageState` following the existing
  `RegistryState` template.
- **Resolved (branch `refactor/audit-tier3-13-17`):** extracted `state::language::LanguageState`
  (5 fields) and `state::models_page::ModelsPageState` (6 fields: the tab bar +
  active-backend selection/staging/menu flags) as `RegistryState`-style sub-structs
  embedded as `AppModel.language` / `AppModel.models_page`; `ModelsPageState::default`
  builds the Installed/Browse tabs. All `self.<field>` / `app.<field>` accesses moved to
  `self.language.<field>` / `self.models_page.<field>`. Model/device/backend-catalog
  state stayed on `AppModel` (touched by `small_state.rs` / the global header).

### [x] 16. 🟡 App: style helpers duplicated across models views

- **Where:** accent-border card closure, glyph tile, and panel style each
  duplicated 2–3× (`surface.rs:94-126` vs `load_sheet.rs:138-157`;
  `active.rs:26-46` vs `load_sheet.rs:24-40`; `models/mod.rs:46-62`,
  `installed.rs:176-203`, `chips.rs:297-311`); older pages still use emoji status
  glyphs vs the newer icon vocabulary (`connection.rs:13-18`,
  `recording.rs:54-63`).
- **Resolved (branch `refactor/audit-tier3-13-17`):** hoisted `accent_border_color(active)`
  and `pill_surface()` into `models/surface.rs` (used by the card + load-sheet row, and
  the header pill + chips track respectively), and parameterized the glyph tile as
  `glyph_tile(tile, glyph, radius_medium)` reused by `backend_glyph_tile` and the
  empty-state ring. **Deferred:** the emoji→icon status-glyph migration
  (`connection.rs`/`recording.rs`) — a visual redesign that needs new SVG assets, not a
  dedup; and the full `card_surface`/overflow-menu unification (would shift radii/shadows).

### [x] 17. 🟡 App: `ui/views/models/download.rs:282` shows `{err:?}` Debug output to users

- **Resolved (branch `refactor/audit-tier3-13-17`):** gave `InstallError` (shared) a
  `Display` impl with human-readable phrasing; the Browse card now shows `Failed: {err}`.

### [x] 18. 🟡 Applet: merge the bar renderers

- **Where:** `equalizer.rs:36-106` and `centered_bars.rs:35-104` differ only in the
  y-anchor; the side-split band selection is triplicated (+`waveform.rs:102-107`).
- **Fix:** one renderer with an anchor enum + `visible_band_range` helper; hoist
  `get_color_with_theme` out of the per-bar loop (recomputed 32×/frame).

### [x] 19. 🟡 Applet: single daemon identity module

- **Where:** AppId/name/scopes defined in both `daemon/client.rs:14-19` and
  `app/subscription.rs:26-28`; display names already disagree, and the
  shared-token-cache invariant is enforced by eyeball.

### [x] 20. 🟡 Applet: one config source of truth

- **Where:** `theme_config` mirrors `config.visualization.*` and both must be
  updated by hand (`app/mod.rs:38,46`, `update.rs:194-203,427-441`); the settings
  UI reads adjacent selectors from different structs (`settings/section.rs:70-71`).
- **Fix:** delete `ThemeConfig`.

### [x] 21. 🟡 Applet: collapse the seven `update_*` config methods

- **Where:** `config/settings.rs:130-183`.
- **Fix:** one closure-based `update()`; `save()` derives the variant from
  `visualization.side` (fixed per binary at load) instead of threading a
  `variant` string, so the `variant_name` field drops entirely.

### [ ] 22. 🟡 Applet: type `icon_alignment`

- **Where:** three hand-written string mappings (`init.rs:22-33`,
  `update.rs:405-419`, `view.rs:68-72`).
- **Fix:** standardize the theme enums on one conversion idiom (currently inherent
  `from_str` / `FromStr` / `From<String>` mixed, with `Display` meaning wire-id for
  some and pretty-name for others).

### [ ] 23. 🟡 Applet: legacy-protocol vestiges (~100 lines)

- Unsendable `RecordingStateChanged`/`AudioLevelUpdate` messages, `PingResponse`
  fields fully discarded, unused `sample_rate` decode, caller-less
  `From<String> for VisualizationColor`, never-constructed
  `IsOpen::AppletSettings`, write-only `UiConfig.last_popup_state`.

### [ ] 24. 🟡 Applet: logging noise

- **Where:** `info!` per successful 5 s ping forever (`update.rs:133`) with stale
  legacy phrasing; the retry path mixes info/warn.
- **Fix:** log transitions at info, steady-state at debug.

### [ ] 25. 🟡 Applet: double clone per frame + stale comment in the visualization `Element` conversion

- **Where:** `sound_visualization.rs:140-148`.

### [ ] 26. 🟡 Shared: dead public API + unused deps

- **Where:** `get_secure_socket_path` + `generate_secure_client_id` (zero
  consumers; `get_http_socket_path` doc still claims dual listeners),
  `device_options!` / `has_cuda_support!` (reference a deleted `cuda` feature);
  unused deps `clap`, `chrono`, `dashmap`, `tokio-stream`; `cpal`/`hound` declared
  for the `audio` feature that only uses `rubato`; the app enables shared's
  `analysis` feature and uses nothing from it. These survive lints because
  pub/macros are exempt.

### [ ] 27. 🟡 Shared: `DaemonResponse` god-struct

- **Where:** 30+ optional fields, ~150 lines of mechanical `with_*` builders;
  `gpu_info: Option<Value>` while both sides use the typed `GpuInfo` in the same
  file (double conversion in `app/daemon/client/v1/settings/backends.rs:116-126`).
- **Fix:** type the field; the broader restructure overlaps Tier 2 #3.

### [ ] 28. 🟡 Shared: `ModelDefinition` is daemon-only yet documented as shared

- **Where:** `models/registry.rs`; it also re-encodes registry-types'
  `Device`/`is_online` invariants stringly.
- **Fix:** move to the daemon or type the field.

### [ ] 29. 🟡 Shared: glob-export shadowing

- **Where:** `pub use models::*` (`lib.rs:11`) is shadowed by top-level
  `registry`/`audio` modules — two same-named module pairs with unrelated content,
  one of each unreachable via the glob.
- **Fix:** rename the inner modules and use explicit re-exports.

### [ ] 30. 🟡 Shared: two audio validators with divergent limits

- **Where:** `utils/audio.rs:29-59` (300 s cap) vs `validation/inputs.rs:80-119` +
  `limits.rs` (30 min); the padding-attack check reports `AudioTooLarge` for a
  content problem.

### [ ] 31. 🟡 Shared: `SUPER_STT_HTTP_SOCKET` is honored only by the daemon

- **Where:** `daemon_main.rs:68-71` — no client reads it, so setting it strands
  every client.
- **Fix:** support it in the client or delete it.

### [ ] 32. 🟡 Indexer: `registry_toml.rs:31-33` doc claims `BTreeMap` preserves file order (it sorts by key)

### [ ] 33. 🟡 Tooling: Cargo convention drift

- **Where:** the indexer redeclares loose versions (`anyhow = "1"`, `clap = "4"`);
  the CLI mixes `workspace = true` with loose pins.
- **Fix:** use `[workspace.dependencies]` throughout.

### [ ] 34. 🟡 Indexer: rename the indexer's `ResolveError` for grep-ability

- The three same-named `ResolveError` enums (custom_repo / local_dir / indexer
  resolve) are distinct concepts, not duplication — at most rename the indexer's.
  Same verdict for `Host` ×2 and `VisualizationConfig` ×2: no action needed.

### [ ] 35. 🟠 Daemon: remaining blocking work (split off Tier 3 #4)

- **Where:** the keyboard `Simulator` path — portal `notify_keysym` spins up an OS
  thread + a fresh current-thread tokio runtime per keysym
  (`xdg_portal_backend.rs:122-153`), enigo sleeps per 64-char chunk /per backspace
  batch (`enigo_backend.rs:24-50`); and the session-store keyring writes
  (`TokenStore::flush_snapshot`/`load_persisted`, `tokens.rs`).
- **Why split:** Tier 3 #4 offloaded the two hot paths that park a runtime worker
  (beep, request-handler keyring). These remaining ones need structure, not a point
  wrap: `Simulator::{type_text,backspace_n}` are sync and driven while the `!Send`
  `actually_typed` `std::Mutex` guard is held across the call, so they want the
  async-`Simulator` rewrite (the portal already holds an async zbus connection, so it
  can drop the thread+runtime entirely). `flush_snapshot` is called from the sync
  `mint`/`revoke`; correctness wants a single-writer persist task (a detached
  `spawn_blocking` per call would race the on-disk ordering), and `load_persisted` is
  a one-time startup blocking read.
- **Fix:** async `Simulator` (threads the `Typer`/preview loop off the `std::Mutex`
  guard); a dedicated session-persist task fed over a channel.

### [ ] 36. 🟡 Daemon: keyring cleanup deferred from Tier 3 #9

- **Where:** the sessions-blob accessors (`keyring.rs` `get_sessions_blob`/
  `set_sessions_blob`) build `keyring::Entry` directly and rely on
  `install_mock_if_requested` (the keyring-crate credential-builder mock), a second
  mock mechanism alongside the process-global `mock_store` that `kv_get`/`kv_set` use.
  Plus the stringly `Result<_, String>` across keyring and download-progress.
- **Why split:** routing the sessions blob through `kv_get`/`kv_set` changes its
  behavior under `SUPER_STT_KEYRING_MOCK` (persists in-process via `mock_store`
  instead of the isolated-per-`Entry` keyring mock), which the `http_smoke_full`
  restart test exercises — a verified change, not a drive-by. Typed errors ripple
  through every keyring/download-progress caller (incl. the Tier 3 #4 async wrappers)
  for marginal benefit.
- **Fix:** route sessions through `kv_get`/`kv_set` and delete
  `install_mock_if_requested`; introduce a small keyring error enum.

### [ ] 37. 🟡 App: roll back optimistic UI state on save failure (split off Tier 3 #11)

- **Where:** the optimistic-then-banner sites — audio theme / feedback
  (`handlers/recording.rs`), volume commit (same), `SelectBackend` /
  `LoadStagedModel` staged device (`handlers/models_page/mod.rs`).
- **Why split:** Tier 3 #11 landed the "one error surface" (scoped banners + Models
  card, no more transcription hijack or log-only). Rollback is separable: the failure
  arrives as its own message that doesn't carry the pre-optimistic value, so each site
  needs a captured previous value threaded through a per-setting success/failure
  message (and volume needs a stored last-committed value, since the drag already
  overwrote `self.volume`). The drift also self-heals on the next reconnect refetch
  (`VolumeLoaded`/`CurrentAudioThemeLoaded`/`ActiveBackendLoaded`).
- **Fix:** capture the prior value at the optimistic set and restore it in a
  dedicated failure message that also raises the banner.

---

## Suggested order of attack

1. **Bug batch** (Tier 1): session reaping (#2), unload persistence (#3), recording
   timeout (#4), uninstall guard (#5), 409 phrases as a stopgap (#1), egress
   allowlist unification (#10), forge download cap (#27).
2. **index.json schema → registry-types** (Tier 2 #1) — highest remaining drift
   risk, proven pattern. *Struct unification has landed; the manifest→index
   synthesis move remains.*
3. **Error codes on `DaemonResponse`** (Tier 2 #3) — one structural change retiring
   findings across four areas.
4. **sha256 / tar / client-factory / atomic-write hardening** (Tier 2 #4).
5. **Wire-enum normalization** (Tier 2 #2) — protocol docs first, per house style.
6. **Shared logging / paths / retry helpers** (Tier 2 #5–#7), then the per-crate
   cleanup backlogs (Tier 3).

---

## Strengths to preserve

- `super-stt-registry-types`: canonical parser with safety guards, schema generated
  from the same types, strong tests — the model for every consolidation above.
- The re-export + policy-layer pattern (`validate_runtime` / indexer `validate`).
- App: `settings_getter!`/`settings_setter!` macros, `require_*` response helpers,
  and the pure-function + unit-test discipline in `status.rs`, `active.rs`,
  `installed.rs`, `events.rs`.
- Daemon: `InflightGuard`, `handle_get_gpu_info`'s `spawn_blocking` pattern, and the
  `HttpError` wire-form pin test — each the right idiom, just not yet applied
  everywhere.
