# Code Quality Audit — July 2026

Full-workspace review covering all nine crates (~54k lines of Rust). Conducted after
the three audit-cleanup passes (#267, #269, #270), so lint-level issues and trivial
dead code were excluded by design; findings below are structural, behavioral, or
contract-level. All findings are open as of `main` on 2026-07-07; file:line
references are to that tree.

---

## 1. Systemic themes

Three patterns account for the majority of findings:

1. **Contracts maintained by hand in N places.** The index.json schema, wire-string
   enums, HTTP status mapping, the consent scope list, and tar/sha256 validation each
   have 2–4 hand-synced copies — and nearly every pair has already drifted in an
   observable way.
2. **Error identity lives in prose.** `DaemonResponse` carries only free-form
   `message` strings. Status codes are derived by substring matching (already broken —
   §2.1), guard messages are re-worded at each call site, and `HttpError`
   classification happens by string comparison at three sites. A machine-readable
   error-code layer retires this entire drift class.
3. **Blocking work on the async runtime.** Typing, beeps, keyring DBus calls, and
   config saves all stall tokio workers; the XDG portal backend builds a fresh
   thread + tokio runtime per keysym.

---

## 2. Confirmed bugs

### Daemon — correctness

2.1. **State-conflict responses return 500 instead of 409.**
`status_code_for_response` (`super-stt-daemon/src/daemon/http/internal/helpers/dispatch.rs:52-106`)
maps response text to status codes by substring, and the phrase list has drifted:
- `switch_guard` messages ("Cannot change the backend during active recording…",
  "…during active real-time transcription sessions.", `daemon/model_management/switch.rs:33-41`)
  match no conflict phrase → 500 on `POST/DELETE /v1/active_backend`, `DELETE /v1/active_model`.
- "Cannot reload the model during active recording…" (`lifecycle.rs:38-44`) → 500.
- "Recording already in progress. Please wait…" (`daemon/recording/mod.rs:44-47`) → 500
  on the `POST /v1/transcribe` race path.
- Two `CONFLICT_PHRASES` are dead: "Cannot switch devices when" and
  "recording in progress" (dispatch.rs:86-87) match no wire string in the crate.

2.2. **Realtime sessions are never removed or cancelled.**
`RealTimeTranscriptionManager` (`super-stt-daemon/src/services/transcription.rs:202-412`)
has no removal/stop API; `cancellation_token` is never cancelled; send failures on a
dead broadcast channel are swallowed. An abandoned session re-runs inference on the
same ~15 s tail every 200 ms indefinitely, and because `switch.rs:37` / `lifecycle.rs:42`
gate on `get_active_sessions().is_empty()`, one dead session permanently blocks model
switching until daemon restart. The throttling fields (`last_emit`, `model_min_interval`)
are written but never read. Also: resampling runs under the sessions write lock
(`transcription.rs:307-321`).

2.3. **`handle_unload_active_model` never persists.**
It clears `preferred_model`/`preferred_source` in memory "so a daemon restart stays
idle" (`lifecycle.rs:184-189`) but never saves — a restart reloads the unloaded model.

2.4. **The recording timeout only stops the preview loop, not the recording.**
`run_preview_loop` breaks after 1 minute (`daemon/recording/preview.rs:114-117`) but
`collect_and_clear_preview` then awaits the recorder indefinitely. With speech detected,
continuous background noise, and `SilenceOnly` stop mode (manual stop refused,
`core.rs:113-130`), capture is unbounded with `busy=true` and a frozen preview.
Fix: send on `manual_stop_tx` on timeout.

2.5. **`DELETE /v1/backends/{source}` skips the mutation guard, strands the loaded
model, and invents a third error envelope.**
(`http/v1/settings/backends.rs:14-87`) No busy/switch guard; never calls
`unload_current_model()` (GPU memory held, `GET /status` and `GET /active_backend`
disagree); returns `{"error": …}` — neither the house `{"status":"error"…}` shape nor
the registry envelope.

2.6. **STT failures are masked as success.**
One-shot transcribe maps backend failure to empty-string success
(`daemon/transcription.rs:156-161`). The recording flow returns
`Ok("[STT error: …]")` (`daemon/recording/mod.rs:122-138`), which write mode types
into the user's focused window, and which HTTP delivers as a success `done` SSE event
despite the contract defining an `error` event (`http/v1/transcribe.rs:170-174`).

2.7. **`POST /v1/transcribe` busy-check TOCTOU on the preview slot.**
Two near-simultaneous requests can both pass the busy check (`transcribe.rs:203`; busy
is set later in `recording/mod.rs:167-175`); the loser unconditionally nulls the
daemon-global preview sender (`transcribe.rs:294`), silently killing the winner's
preview stream. Claim the slot under the same write that sets `busy`, and clear only
if it is still your sender.

2.8. **`--no-default-features --features wasm-backends` does not compile** (verified
via `cargo check`): the subprocess stub is missing `&self`
(`daemon/model_management/instantiate.rs:159-169`), and
`http/v1/registry/install.rs:32,35` references variants that don't exist in that
feature set. Neither combination is exercised by CI.

2.9. **Unsolicited `304` panics the registry client.**
`refresh()` does `self.state.read().as_ref().unwrap()` on 304
(`registry/client.rs:147-152`); a misbehaving proxy can panic the daemon.
`Client::new` also unwraps the builder (`client.rs:55-63`).

2.10. **HTTP vs WS egress allowlists diverge in the wasm host** *(security)*.
`send_request` matches the authority as written (`stt_models/wasm/host.rs:67-69`)
while `check_host_allowed` matches synthesized `host:port` (`host.rs:121-124`), so
`"api.example.com:443"` matches wss but not https, despite `host.rs:106-107`
documenting identical rules. The hook also does synchronous DNS on the runtime and is
check-then-connect (TOCTOU, acknowledged at `host.rs:84-85`). Route `send_request`
through `check_host_allowed`.

2.11. **Duplicate adaptive-VAD in `RealTimeSession::add_audio_chunk` reintroduces the
NaN hazard fixed in #268 and feeds write-only state.**
(`services/transcription.rs:90-132`) RMS computes 0/0 = NaN on an empty chunk (the
guard exists only in `audio/state.rs:78-81`); `recent_levels`/`silence_start`/votes
are never read. Delete the block or reuse `RecordingState`.

2.12. **Preview typer accounting mixes bytes and chars.**
`apply_simple_diff` returns bytes (`output/typer.rs:200,231`) that `apply_text_update`
adds to char counts (`typer.rs:313-350`); deletions never subtracted; the mismatch
warn (`typer.rs:361-373`) fires spuriously on non-ASCII and both branches do the same
thing. Also an unconditional "Failed to type" debug line (`typer.rs:199`) and an
`i32`/`-1` sentinel API (`find_tail_match_in_text`, `output/preview.rs:72`) with
O(n²) `Vec<char>` rebuilds.

### App

2.13. **A single failed `set_volume`/theme POST flips the app to the full-screen
connection-error page.** Volume/theme failures map to `Message::DaemonError`
(`handlers/recording.rs:100-124`), and `view.rs:140-145` forces the Connection page
whenever not `Connected`.

2.14. **The app refetches all settings every 5 seconds forever.**
Successful pings map to `Message::DaemonConnected` (`handlers/daemon/mod.rs:79-84`),
whose handler unconditionally runs six settings GETs + language load
(`daemon/mod.rs:131-142, 243-302`). Settings-save successes reuse the same ack, so
each toggle triggers a full refetch. Introduce a dedicated `SettingSaved` ack and load
settings only on the disconnected→connected transition (SSE `settings_changed` already
covers cross-client sync).

2.15. **Failed secret/option saves are invisible.** All backend secret/option save
failures route to log-only `BackendsError` (`handlers/backend.rs:103-211`) — a failed
API-key save shows nothing in the Configure sheet. `InstallFailedToStart` is likewise
invisible (`handlers/models_page/install.rs:69-73`). Optimistic values are never
rolled back (`handlers/settings.rs:46,62-65`).

2.16. **`$HOME` sanitization corrupts messages when HOME is unset.**
`err.replace(&std::env::var("HOME").unwrap_or_default(), "$HOME")`
(`handlers/model.rs:120-124`) — empty pattern inserts `$HOME` at every char boundary.

2.17. **`handle_daemon_events` returns from inside the loop**
(`handlers/daemon/events.rs:16-34`), dropping all events after the first that yields a
task. Latent today (producers wrap singletons) but the API takes a `Vec`.

2.18. **Reconnect force-navigates to Customization**
(`handlers/daemon/mod.rs:117-129`), contradicting the documented Models launch page
(`core/app/init.rs:18-24`) and yanking mid-flow users on daemon restart.

2.19. **Volume slider fires one POST per drag tick**
(`ui/views/customization.rs:56`, `handlers/recording.rs:120-126`) — commit on
`on_release` or debounce.

2.20. **Language client encodes `source` but interpolates `model` raw**
(`daemon/client/v1/settings/language.rs:67,89,116`) — a model name containing `/`
produces a malformed path.

### Applet

2.21. **Empty `frequency_bands` panics/degenerates the bar renderers.**
`b64_to_f32_vec` degrades malformed input to an empty vec, but
`equalizer.rs:60` / `centered_bars.rs:59` compute `bars_to_show - 1` → usize underflow
(debug panic; ~1.8e19 spacing in release). Same class as the #268 fix. Guard
`bands_to_show == 0` and use `saturating_sub`.

2.22. **`launch_app`/`open_github` never reap children** (`app/update.rs:348-389`) —
zombies accumulate for the session-long applet — and `launch_app` probes hardcoded
`./target/{debug,release}/` dev paths relative to the panel process CWD.

2.23. **The "connection health watchdog" doesn't exist.** `last_udp_data` is written
four times, read nowhere (`app/mod.rs:45`, `update.rs:47,57,229-231,256`); the
comments advertise a safety net that was removed.

### Shared / registry-types

2.24. **`GET /audio_themes` violates the documented wire form.** `AudioTheme` derives
serde with no `rename_all` (`models/theme.rs:7-31`) and is put on the wire via
`available_audio_themes` (`protocol/response.rs:49`), returning
`["Classic","SciFi",…]` where `docs/protocol/endpoints/v1/audio_themes.md` pins
lowercase. Unnoticed because the smoke test only asserts `is_array()`.

2.25. **`SubprocessAsset` with `file = ""` plus valid `parts` passes `Manifest::parse`**
(XOR guard treats empty as absent, `super-stt-registry-types/src/manifest.rs:528-534`)
but `release_files()` returns `[""]` and `is_multipart()` is false
(`manifest.rs:202-213`). Normalize empty→`None` in parse.

2.26. **`cmd_record` silently drops an invalid `stop_mode`**
(`.ok()`, `protocol/dispatch.rs:123-128`) while `set_recording_stop_mode` 400s the
same input (`dispatch.rs:237-248`); the `disable_silence_detection` compat branch
(`dispatch.rs:129-142`) is undocumented and caller-less.

### Tooling (indexer / forge / consent)

2.27. **`ForgeClient::download` buffers unbounded bodies** *(security)*.
`github.rs:143-147` reads the whole body; `custom_repo.rs` size-checks only declared
(attacker-controlled) metadata before download and the real cap only after. Add
`max_bytes` to the trait, stream with an accumulating check (the pattern already
exists at `registry/install.rs:633-650` and `indexer/src/assets.rs:144-171`).

2.28. **One malformed `repo` string aborts the entire index build.**
`RepoRef::parse(&entry.repo)?` (`indexer/src/main.rs:96`) bypasses the
carry-forward resilience path every other per-entry failure uses.

2.29. **Multi-GB temp parts leak on mid-loop errors**
(`indexer/src/main.rs:246-262`) — `?` returns before the cleanup loop. Use a
`TempDir`/`Drop` guard.

2.30. **The consent auto-approve env path ships in release builds.**
`STT_AUTH_AUTO_APPROVE_AFTER_MS` (`consent/src/main.rs:288-318`) makes the human
trust gate self-approve. Gate behind `#[cfg(debug_assertions)]` or a test feature.

2.31. **Daemon "update" uses string equality, so it happily downgrades**
(`registry/update.rs:167`), where the app's check is strictly-newer semver
(`app/.../installed.rs:42-49`); the indexer has two more subtly different
`v`-prefix strippers (`resolve.rs:96-98`, `manifest.rs:45`). One shared
`parse_version`/`update_available`.

---

## 3. Cross-crate standardization targets

Ranked by drift risk. These answer "what should be standardized or reused."

3.1. **index.json schema + manifest→index mapping → `super-stt-registry-types`.**
Nine structs exist twice — producer (`indexer/src/index_json.rs:14-125`,
Serialize) and consumer (`daemon/src/registry/index_schema.rs:48-195`, Deserialize) —
sync'd by comment only, with `IndexStale` a third time
(`shared/src/registry/mod.rs:86`). Confirmed drift: secret labels fall back to the
secret name (indexer `main.rs:326`) vs `""` (daemon `custom_repo.rs:145,154`); option
`type` defaults `"string"` vs `""` (`custom_repo.rs:155`); option `default` is
`expect()` vs silently dropped; `license` required vs `#[serde(default)]`. The same backend renders
differently depending on install path. Use the proven layering: canonical types in
registry-types, daemon keeps `check_min_client`/`retain_safe_backends` as extensions
(like `validate_runtime` for `Manifest`). Fold the field-identical
`RegistryModel`/`RegistrySecret`/`RegistryOption` (`shared/src/registry/mod.rs:41-91`)
into the same leaf types. Also move the manifest→`IndexBackend` synthesis
(`into_index_backend`, `classify_models`, id-from-source) shared by
`custom_repo.rs:175-209` / `local_dir.rs:59-117` / `indexer/main.rs:273-351` — note
`local_dir` silently drops `secrets`/`options` (`local_dir.rs:107-108`) where
`custom_repo` maps them.

3.2. **One wire-string enum convention.** `RecordingStopMode`, `WriteMethod`, and
`AudioTheme` each carry 2–3 live string forms (PascalCase serde persisted in
daemon.toml via `config.rs:53-100`, kebab-case `Display` on the response wire,
aliased `FromStr` on input) with three unknown-value policies (silent default /
hard error / hard error) and undocumented aliases (`silence|both|manual`,
`xdg|portal|wayland`) that violate the no-legacy-aliases rule. One macro generating
serde/Display/FromStr from a single table — snake_case, fallback-to-default, no
aliases — with `deserialize_or_default` covering config migration. Requires a
protocol-docs pass first (docs currently pin kebab-case; decide kebab vs snake
deliberately). Includes fixing 2.24.

3.3. **Machine-readable error codes on `DaemonResponse`.** Retires: the substring
status mapping (2.1), the four re-worded switch guards
(`switch.rs:31-43`, `switch.rs:259-282`, `lifecycle.rs:37-45`,
`device_management.rs:157-180` → one `guard_model_mutation(action)`), the
`HttpError` string classification (`shared/daemon/session.rs:236`,
`widget_subscription/mod.rs:188-193`, `app/core/app/events.rs:20` — root cause:
`with_token`'s `op` erases to `Result<T, String>`; have it return `HttpResult<T>`),
and the ad-hoc error envelopes (2.5).

3.4. **Download/verify plumbing.**
- `verify_sha256`: `stt_models/download.rs:64,185` compares case-insensitively;
  `registry/install.rs:432,499,602` compares `==`. One helper.
- Tar entry-safety + budgets: identical escape checks duplicated
  (`install.rs:522-532` vs `indexer/assets.rs:197-213`), but the daemon's per-entry
  4 GiB cap and zip-bomb budget (`install.rs:27,536-556`) are missing at publish
  time — a green publish can fail every install. Extract predicate + budgets into
  registry-types and run install policy in the indexer.
- HTTP client factory: five builders, four styles — forge (`github.rs:73-77`,
  expect), registry client (`client.rs:58-62`, unwrap, no UA), model download
  (`download.rs:220-223`), pipeline (`pipeline.rs:113-123`, unwrap_or_default), and
  the indexer's bare `Client::new()` (`indexer/main.rs:85`) — **no timeout at all**.
  One `short_client()`/`download_client()` with a workspace UA.
- Atomic writes: three tmp+rename variants in the daemon (`install.rs:344-359`,
  `registry/client.rs:170,198`, `download.rs:191`); the indexer writes `index.json`
  non-atomically and its two modes disagree on a trailing newline
  (`main.rs:131-133` vs `local.rs:67-70`). One `write_atomic` helper.
- Within `install.rs`, `stream_download` (`:366-416`) and `download_verified_parts`
  (`:443-511`) duplicate the chunk loop; `stt_models/download.rs:101-193` is a third
  implementation with better conventions (tmp+rename, cancellation, sync_all).
  Extract `stream_to_file(http, url, cap, dest, on_chunk) -> sha256`.

3.5. **Daemon connection supervisor + retry policy.** Three reconnect policies against
the same daemon: shared `next_backoff` (doubling 1s→30s, no jitter), applet
`RetryStrategy` (`applet/src/daemon/retry.rs:12-60`, exponential + jitter — the best),
app flat 5 s sleep (`handlers/daemon/mod.rs:158-166`). The applet's subscription
bridge, `RetryAuthorization` handling, ping cycle, and even the scope-coverage test
are a hand-maintained fork of the app's (`applet subscription.rs:20-63`,
`update.rs:116-165,280-346` vs `app subscription.rs:11-101`,
`handlers/daemon/mod.rs:74-208`). Promote `RetryStrategy` and a generic subscription
bridge into shared; per-app topics/scopes/messages stay per-crate.

3.6. **Logging init.** Five variants: byte-identical RUST_LOG-else-Info blocks in
`app/main.rs:9-17` and `consent/main.rs:341-347`, parameterized in
`daemon_main.rs:18-33`, one-liner in `indexer/main.rs:64`, bare `env_logger::init()`
in the applet (defaults to **Error** — effectively silent vs siblings), and nothing
at all in the CLI. One `shared::logging::init(default_level)`. Also: the daemon loads
config *before* initializing logging (`daemon_main.rs:93` vs `:114`), so the
"config invalid, reset to defaults" warning (`config.rs:173`) is silently dropped —
init logging first.

3.7. **XDG path helpers.** `get_config_path` is byte-identical daemon↔applet
(`daemon/config.rs:154-163` vs `applet/config/settings.rs:67-76`); three
incompatible `dirs`-miss fallbacks coexist (HOME-else-/tmp, `env::temp_dir()`, raw
`XDG_RUNTIME_DIR`); and the subprocess socket path
(`stt_models/subprocess/mod.rs:103-106`) rebuilds `$XDG_RUNTIME_DIR/stt/…` while
bypassing the validated `secure_socket_path` helper
(`shared/validation/paths.rs:29-61`). One `shared::paths` module; route the
subprocess socket through the validated helper.

3.8. **Smaller shared items.**
- `accept_base_url` duplicated verbatim (`forge/lib.rs:139-147` vs
  `daemon/registry/mod.rs:17-22`) — keep forge's, import it.
- CryptoProvider install ×3 (+1 redundant call at `download.rs:219`); one
  `install_crypto_provider()` beside the client factory.
- Consent scope list hand-mirrors the daemon's `KNOWN_SCOPES`
  (`consent/main.rs:200-212` vs `responses.rs:96-105`); a new scope renders the
  "deny is safe" warning on legitimate prompts. Shared leaf data + conformance test.
- CLI hard-codes stop-mode strings (`cli/main.rs:68`); derive `clap::ValueEnum` on
  the shared enum.
- SSE framing loop duplicated in the shared client
  (`http_client/v1/transcribe.rs:131-162` vs `events.rs:82-122`) — generic
  `sse::block_stream`; also fix the stale NDJSON doc comments in transcribe.rs.
- Hand-rolled `unsafe` pin projection (`http_client/internal/transport.rs:17-42`)
  re-implements `http_body_util::Either` — the crate's only `unsafe`, deletable.

---

## 4. Per-crate cleanup backlogs

### super-stt-daemon

- **Wasm/subprocess backends duplicate the `/v1` client** (~35 lines already drifted:
  `wasm/mod.rs:400-443` vs `subprocess/mod.rs:324-361`, plus `invoke` vs `request`,
  `status`/`ping` vs `wait_for_ping`). Extract `build_transcribe_body` /
  `parse_transcribe_response` / a small `V1Transport` trait.
- **Device-switch success/recovery duplicate ~120 lines** and both bypass
  `unload_current_model()`'s graceful shutdown, dropping the model under the write
  lock (`device_management.rs:236-321,361-421` vs `switch.rs:229-257`). One
  `finalize_loaded_model()`; route unload through the real path.
- **Config persistence has three idioms**: self-saving `update_*` (blocking
  `fs::write` under the tokio config write lock, errors swallowed), pure mutation +
  `persist_config()`, and both-at-once (double/triple writes:
  `switch.rs:243-249`, `device_management.rs:113-118,288-296`,
  `theme_handlers.rs:30-39,74-83`) — plus neither (bug 2.3). Make all mutators pure,
  persist via `persist_config()` in `spawn_blocking`; `save()`'s
  `Box<dyn Error>` → `anyhow::Result`.
- **Blocking on the runtime**: portal backend spawns a thread + new tokio runtime per
  keysym then joins (`xdg_portal_backend.rs:122-153`, two runtime `expect`s); enigo
  sleeps per chunk; `play_beep_sequence` spin-waits whole beep durations and
  `handle_test_audio_theme` calls it inline (`theme_handlers.rs:96-161`); keyring
  DBus inline in auth middleware and secret endpoints (`tokens.rs:198-230`,
  `secrets.rs:34`, `backend_config_handlers.rs:145,162`). Make `Simulator` async
  (portal already holds an async zbus connection) or `spawn_blocking` throughout;
  `handle_get_gpu_info` (`device_management.rs:460-468`) shows the right pattern.
- **Mutex poison recovery copy-pasted ~10×** in `audio/`
  (`recorder.rs`, `processing.rs`, `device.rs`), while the rest of the crate uses
  parking_lot. Switch audio to `parking_lot::Mutex` (cpal callbacks are sync-safe
  with it) or one `lock_recover` helper.
- **Inflight cleanup hand-rolled in 8+ places despite an RAII guard**:
  `install_inflight.write().remove()` on every error path
  (`install.rs:149-241`, `update.rs:70-168`) while `pipeline.rs`'s `InflightGuard`
  is only used inside the spawned task. Construct the guard at insert.
- **Settings handlers repeat a 25-line mutate→persist→respond block 6×**
  (`settings_handlers.rs:12-250`); one `set_config_field` helper (also prevents 2.3
  recurrences).
- **SSE fan-out uses unbounded channels** (`http/v1/events.rs:75-76,164-190`) — a
  stalled reader buffers `frequency_bands` frames without bound; use a bounded
  channel and drop visualization frames on overflow.
- Minor: five identical `emit_*` DBus wrappers (`services/dbus.rs:131-198`); dead
  spinner scaffolding in `transcribe_with_spinner` (`recording/transcribe.rs:64-121`);
  `PipeExt` trait for one `.pipe(Ok)` (`recorder.rs:580-593`); keyring sessions-blob
  accessors bypass `kv_get`/`kv_set` with a second mock mechanism
  (`keyring.rs:157-247`); stringly `Result<_, String>` in keyring/download-progress.

### super-stt-app

- **Group the flat ~90-variant `Message` enum into sub-enums.** Routing is declared
  twice (nine `matches!` lists in `core/app/update.rs:21-211` + per-handler
  `_ => Task::none()` catch-alls); a forgotten variant silently no-ops. Sub-enums
  make dispatch exhaustive and delete both lists.
- **One error surface.** Four ad-hoc patterns today (transcription-box hijack,
  log-only, invisible, escalate-to-connection-page — bugs 2.13/2.15). The
  `ModelError` → in-card banner path (`handlers/model.rs:118-131`,
  `ui/views/models/active.rs:437-445`) is the good template; add a shared
  scope-tagged error slot rendered per page, and roll back optimistic state on
  failure.
- **`clear_loaded_model()` helper**: the triple-assignment
  (`current_model`/`current_provider`/`current_source`) is copy-pasted at seven
  sites, each hand-picking adjacent resets (`handlers/models_page/mod.rs:144-149,
  281-284, 299-302`, `handlers/model.rs:128-130`, `handlers/download.rs:115-127`,
  `core/app/small_state.rs:95-97`).
- **Shared task builders**: registry catalog fetch, `list_backends` reload, and ping
  are re-rolled at 3–4 sites each (`fetch_registry_catalog()`, `reload_backends()`,
  `ping_task()` beside the existing `build_load_settings_tasks()`).
- **Type the language payload**: `model_language: Option<serde_json::Value>` parsed
  field-by-field in two views (`core/app/mod.rs:162-168`,
  `ui/views/models/active.rs:100-125`, `ui/views/language_picker.rs:25-35`);
  deserialize a `LanguageResolution` struct at the client boundary.
- **Split `AppModel`** (~45 fields): extract `ModelsPageState` and `LanguageState`
  following the existing `RegistryState` template.
- **Style helpers**: accent-border card closure, glyph tile, and panel style each
  duplicated 2–3× across models views (`surface.rs:94-126` vs
  `load_sheet.rs:138-157`; `active.rs:26-46` vs `load_sheet.rs:24-40`;
  `models/mod.rs:46-62`, `installed.rs:176-203`, `chips.rs:297-311`); older pages
  still use emoji status glyphs vs the newer icon vocabulary
  (`connection.rs:13-18`, `recording.rs:54-63`).
- Minor: `ui/views/models/download.rs:282` shows `{err:?}` Debug output to users.

### super-stt-cosmic-applet

- **Merge the bar renderers**: `equalizer.rs:36-106` and `centered_bars.rs:35-104`
  differ only in the y-anchor; the side-split band selection is triplicated
  (+`waveform.rs:102-107`). One renderer with an anchor enum + `visible_band_range`
  helper; hoist `get_color_with_theme` out of the per-bar loop (recomputed 32×/frame).
- **Single daemon identity module**: AppId/name/scopes defined in both
  `daemon/client.rs:14-19` and `app/subscription.rs:26-28`; display names already
  disagree, and the shared-token-cache invariant is enforced by eyeball.
- **One config source of truth**: `theme_config` mirrors `config.visualization.*`
  and both must be updated by hand (`app/mod.rs:38,46`, `update.rs:194-203,427-441`);
  the settings UI reads adjacent selectors from different structs
  (`settings/section.rs:70-71`). Delete `ThemeConfig`.
- **Collapse the seven `update_*` config methods** (`config/settings.rs:130-183`)
  into one closure-based `update()`; store `variant` in the struct.
- **Type `icon_alignment`**: three hand-written string mappings
  (`init.rs:22-33`, `update.rs:405-419`, `view.rs:68-72`); standardize the theme
  enums on one conversion idiom (currently inherent `from_str` / `FromStr` /
  `From<String>` mixed, with `Display` meaning wire-id for some and pretty-name for
  others).
- **Legacy-protocol vestiges** (~100 lines): unsendable
  `RecordingStateChanged`/`AudioLevelUpdate` messages, `PingResponse` fields fully
  discarded, unused `sample_rate` decode, caller-less `From<String> for
  VisualizationColor`, never-constructed `IsOpen::AppletSettings`, write-only
  `UiConfig.last_popup_state`.
- **Logging noise**: `info!` per successful 5 s ping forever (`update.rs:133`) with
  stale legacy phrasing; retry path mixes info/warn. Log transitions at info,
  steady-state at debug.
- Minor: double clone per frame + stale comment in the visualization `Element`
  conversion (`sound_visualization.rs:140-148`).

### super-stt-shared / super-stt-registry-types

- **Dead public API + unused deps** (survive lints because pub/macros are exempt):
  `get_secure_socket_path` + `generate_secure_client_id` (zero consumers;
  `get_http_socket_path` doc still claims dual listeners), `device_options!` /
  `has_cuda_support!` (reference a deleted `cuda` feature); unused deps `clap`,
  `chrono`, `dashmap`, `tokio-stream`; `cpal`/`hound` declared for the `audio`
  feature that only uses `rubato`; the app enables shared's `analysis` feature and
  uses nothing from it.
- **`DaemonResponse` god-struct**: 30+ optional fields, ~150 lines of mechanical
  `with_*` builders; `gpu_info: Option<Value>` while both sides use the typed
  `GpuInfo` in the same file (double conversion in
  `app/daemon/client/v1/settings/backends.rs:116-126`) — type it.
- **`ModelDefinition` is daemon-only** (`models/registry.rs`) yet documented as
  shared, and re-encodes registry-types' `Device`/`is_online` invariants stringly.
  Move to the daemon or type the field.
- **Glob-export shadowing**: `pub use models::*` (`lib.rs:11`) is shadowed by
  top-level `registry`/`audio` modules — two same-named module pairs with unrelated
  content, one of each unreachable via the glob. Rename the inner modules and use
  explicit re-exports.
- **Two audio validators with divergent limits**: `utils/audio.rs:29-59` (300 s cap)
  vs `validation/inputs.rs:80-119` + `limits.rs` (30 min); the padding-attack check
  reports `AudioTooLarge` for a content problem.
- `SUPER_STT_HTTP_SOCKET` is honored only by the daemon (`daemon_main.rs:68-71`) —
  no client reads it, so setting it strands every client. Support it in the client
  or delete it.

### super-stt-indexer / forge / consent / cli

Covered by 2.27–2.31 and 3.4/3.6/3.8, plus:
- Indexer `registry_toml.rs:31-33` doc claims `BTreeMap` preserves file order (it
  sorts by key).
- Cargo convention drift: indexer redeclares loose versions (`anyhow = "1"`,
  `clap = "4"`), CLI mixes `workspace = true` with loose pins — use
  `[workspace.dependencies]` throughout.
- Three same-named `ResolveError` enums (custom_repo / local_dir / indexer resolve)
  are distinct concepts, not duplication — at most rename the indexer's for
  grep-ability. Same verdict for `Host` ×2 and `VisualizationConfig` ×2.

---

## 5. Suggested order of attack

1. **Bug batch** (§2): session reaping (2.2), unload persistence (2.3), recording
   timeout (2.4), uninstall guard (2.5), 409 phrases as a stopgap (2.1), egress
   allowlist unification (2.10), forge download cap (2.27).
2. **index.json schema → registry-types** (3.1) — highest remaining drift risk,
   proven pattern.
3. **Error codes on `DaemonResponse`** (3.3) — one structural change retiring
   findings across four areas.
4. **sha256 / tar / client-factory / atomic-write hardening** (3.4).
5. **Wire-enum normalization** (3.2) — protocol docs first, per house style.
6. **Shared logging / paths / retry helpers** (3.5–3.7), then the per-crate dedup
   backlogs (§4).

---

## 6. Strengths to preserve

- `super-stt-registry-types`: canonical parser with safety guards, schema generated
  from the same types, strong tests — the model for every consolidation above.
- The re-export + policy-layer pattern (`validate_runtime` / indexer `validate`).
- App: `settings_getter!`/`settings_setter!` macros, `require_*` response helpers,
  and the pure-function + unit-test discipline in `status.rs`, `active.rs`,
  `installed.rs`, `events.rs`.
- Daemon: `InflightGuard`, `handle_get_gpu_info`'s `spawn_blocking` pattern, and the
  `HttpError` wire-form pin test — each the right idiom, just not yet applied
  everywhere.
