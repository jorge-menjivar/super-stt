# Settings App Code-Quality Refactor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring every file in `super-stt-app/src/` under ~500 lines, remove all `#[allow(clippy::too_many_lines)]`, delete AI/mockup narration comments, and de-duplicate proven repetition — with zero behaviour change.

**Architecture:** Two oversized files (`core/app.rs` 2791 lines, `ui/views/models.rs` 2451 lines) are split into directory modules. `core/app.rs` becomes `core/app/` with child modules so moved `impl AppModel` handler blocks retain access to `AppModel`'s private fields (`core`, `context_page`, `nav`). The `cosmic::Application` trait impl stays contiguous in `app/mod.rs` and delegates its two large methods (`init`, `update`) to inherent methods in child files. `ui/views/models.rs` becomes `ui/views/models/` split by widget, with reusable primitives (formatting, container chrome, chips) lifted into their own modules. Work proceeds in 4 phases, one commit per phase.

**Tech Stack:** Rust, `libcosmic` (COSMIC desktop toolkit), `cargo clippy`/`cargo test`, `tokei`.

---

## Reference: source layout (pre-refactor)

`core/app.rs` (2791 lines) contains, in order:
- L30–58: `ModelOperationState`, `DeviceState` enums
- L62–177: `AppModel` struct (`core`, `context_page`, `nav` are **private**; all other fields `pub`)
- L179–862: `impl cosmic::Application for AppModel` — `core`/`core_mut` (192–197), `init` (202–345), `header_start` (346–365), `header_end` (366–385), `nav_model` (386–395), `context_drawer` (396–446), `view` (447–489), `subscription` (490–520), `update` (521–848, a dispatcher + inline tail at 651–846), `on_nav_select` (851–861)
- L864–2449: `impl AppModel` inherent methods — `is_model_ready` (866), `set_model_downloading` (871), `set_model_loading` (883), `set_device_switching` (891), `handle_daemon_messages` (900–1359), `handle_device_messages` (1360–1436), `handle_download_messages` (1437–1551), `handle_model_messages` (1553–1711), `handle_preview_typing_messages` (1712–1741), `handle_recording_stop_mode_messages` (1742–1773), `handle_write_method_messages` (1774–1802), `handle_backend_messages` (1803–1917), `reload_if_active_backend` (1918–1933), `backend_model_provider` (1934–1948), `handle_models_page_messages` (1949–2422), `update_title` (2423–2449)
- L2451–2683: free fns `classify_daemon_error`, `audio_events_subscription`, `settings_widget_event_to_message`, `widget_event_to_notification`, `raw_level_to_db_display_percent`
- L2684–2701: `impl menu::action::MenuAction for MenuAction`
- L2703–end: `#[cfg(test)] mod tests`

`update()`'s inline tail (651–846) directly handles: `CustomModelsDir*` (652–695), template msgs `OpenRepositoryUrl`/`ToggleContextPage`/`LaunchUrl` (714–732), recording msgs `StartRecording`/`StopRecording`/`PreviewTextReceived`/`TranscriptionReceived` (735–778), audio msgs `AudioLevelUpdate`/`AudioFeedbackToggled`/`AudioThemeSelected`/`SetAudioTheme`/`AudioThemesLoaded`/`VolumeChanged` (780–824), widget msgs `WidgetAudioLevel`/`WidgetRecordingState`/`RecordingStateChanged` (826–842).

`ui/views/models.rs` (2451 lines) is all free functions taking `&AppModel` (no private-field issue — split is plain function moves). Function boundaries are listed inline in Phase C tasks.

**`#[allow(clippy::...)]` inventory:**
- `core/app.rs`: `struct_excessive_bools` (62, keep+justify), 7× `too_many_lines` (201, 521, 899, 1436, 1552, 1800, 1948 — all removed by splitting), `cast_possible_truncation` (2569, keep+justify)
- `ui/views/common.rs`: `elidable_lifetime_names` (26, keep+justify)
- `ui/views/models.rs`: 4× `cast_precision_loss` (200, 208, 284, 1289 — keep+justify), `similar_names` (729, keep+justify)

---

## Phase A — Comment & allow audit (1 commit)

No structural changes. Comments and `#[allow]` lines only.

### Task A1: Capture baseline

**Files:** none modified.

- [ ] **Step 1: Record per-file line counts and clippy output**

Run:
```bash
cd /home/jorge/rust_projects/super-stt
find super-stt-app/src -name '*.rs' -exec wc -l {} \; | sort -rn > /tmp/baseline_loc.txt
cargo clippy --all-features --workspace -- -W clippy::pedantic -D warnings -D unused_must_use 2>&1 | tee /tmp/baseline_clippy.txt
cargo test -p super-stt-app --bins 2>&1 | tail -20 | tee /tmp/baseline_test.txt
```
Expected: clippy passes (exit 0); tests pass. These files are scratch references, never committed.

### Task A2: Remove mockup comments from `ui/views/models.rs`

**Files:**
- Modify: `super-stt-app/src/ui/views/models.rs`

The mockup references are at lines 107, 403, 842, 1068, 1109, 1205, 1346, 1385, 1392, 1418, 1434, 1436, 1509, 1510, 1544, 1561, 1698 (per `grep -n mockup`). Each is a decorative cross-reference to a private design mockup.

- [ ] **Step 1: Delete each mockup reference**

For each occurrence, apply this rule:
- If the comment is **only** a mockup cross-reference (e.g. `// matches the mockup's .fgroup`), delete the whole comment line (or the mockup clause if it's part of a larger sentence that still carries meaning).
- If the comment mixes a mockup reference with a real reason, keep the reason and strip the mockup clause. Example: `// Two backends per row, matching the mockup's grid-template-columns` → `// Two backends per row.`

Verify after edits:
```bash
grep -rn 'mockup' super-stt-app/src/
```
Expected: no output.

- [ ] **Step 2: Verify still compiles**

Run: `cargo build -p super-stt-app`
Expected: success (comment-only changes).

### Task A3: Sweep remaining AI-narration comments

**Files:**
- Modify: `super-stt-app/src/` (any file with matches)

- [ ] **Step 1: Find narration candidates**

Run:
```bash
grep -rn -E '//\s*(For now|Note that|Note:|We don'"'"'t|We now|This is just|Original template)' super-stt-app/src/
```
Known hits include `core/app.rs:1149` (`// Note: "device_switched" event handler removed...`), `core/app.rs:1375` (`// We don't verify with get_device...`), `core/app.rs:713` (`// Original template messages`).

- [ ] **Step 2: Apply the keep/delete rule to each**

Keep a comment **only** if removing it would hide a non-obvious invariant the code itself can't express. Delete pure narration.
- `// Original template messages` → delete (decorative section header).
- `// Note: "device_switched" event handler removed - we now only use "ready" events` → delete (describes history, not current invariant).
- `// We don't verify with get_device to avoid premature requests` → **keep** (documents a deliberate non-obvious choice — a hidden "why").

Use judgement per the rule; do not bulk-delete.

- [ ] **Step 3: Verify compiles**

Run: `cargo build -p super-stt-app`
Expected: success.

### Task A4: Justify the `#[allow]`s that stay

**Files:**
- Modify: `super-stt-app/src/core/app.rs`, `super-stt-app/src/ui/views/models.rs`, `super-stt-app/src/ui/views/common.rs`

Do **not** touch the 7 `too_many_lines` allows — those are removed in Phase B.

- [ ] **Step 1: Add/refine a `// reason:` line above each non-`too_many_lines` allow**

- `core/app.rs:62` `#[allow(clippy::struct_excessive_bools)]` → add above it: `// reason: AppModel mirrors discrete UI toggles; COSMIC apps accumulate independent bool flags.`
- `core/app.rs:2569` `#[allow(clippy::cast_possible_truncation)]` → add a `// reason:` describing why the truncation is safe (inspect the surrounding cast and state the bound).
- `ui/views/common.rs:26` `#[allow(clippy::elidable_lifetime_names)]` → add a `// reason:` (inspect why the explicit lifetime is needed for clarity/required by signature).
- `ui/views/models.rs:200,208,284,1289` `#[allow(clippy::cast_precision_loss)]` already carry `// display-only` notes; normalise them to `// reason: display-only; the imprecision is cosmetic`.
- `ui/views/models.rs:729` `#[allow(clippy::similar_names)]` already carries a note; normalise to `// reason: "supports_gpu" / "supports_cpu" are the clearest names`.

- [ ] **Step 2: Verify clippy still clean**

Run: `cargo clippy --all-features --workspace -- -W clippy::pedantic -D warnings -D unused_must_use`
Expected: passes.

### Task A5: Commit Phase A

- [ ] **Step 1: Run full gate**

Run:
```bash
cargo clippy --all-features --workspace -- -W clippy::pedantic -D warnings -D unused_must_use
cargo test -p super-stt-app --bins
```
Expected: both pass.

- [ ] **Step 2: Commit** (only after the user has authorised committing — see "Commit policy" at end)

```bash
git add super-stt-app/src/
git commit -m "Remove mockup/AI narration comments and justify remaining clippy allows in settings app"
```

---

## Phase B — Split `core/app.rs` (1 commit)

Convert `core/app.rs` (file) → `core/app/` (directory module). Child modules can read `AppModel`'s private fields because privacy grants access to the defining module **and all its descendants**.

### Target layout
```
core/
  mod.rs                    unchanged: `pub mod app; pub use app::AppModel;`
  app/
    mod.rs            ~420  AppModel struct + ModelOperationState/DeviceState enums
                            + `impl cosmic::Application for AppModel` (init & update delegate)
                            + `impl menu::action::MenuAction for MenuAction`
                            + module declarations for the children below
    init.rs           ~180  inherent `fn init_model(core, flags) -> (Self, Task)`
    view.rs           ~200  inherent header_start/header_end/nav_model/context_drawer/view helpers
    subscription.rs   ~150  inherent subscription helper + free fn audio_events_subscription
    update.rs         ~280  inherent `fn dispatch(&mut self, msg) -> Task` (the matches!-router)
    events.rs         ~260  free fns classify_daemon_error, settings_widget_event_to_message,
                            widget_event_to_notification, raw_level_to_db_display_percent + tests
    small_state.rs    ~60   inherent is_model_ready/set_model_downloading/set_model_loading/
                            set_device_switching/update_title
    handlers/
      mod.rs          ~10   `pub(super) mod ...;` declarations
      daemon.rs       ~400  handle_daemon_messages, split into 2-3 private sub-fns
      device.rs       ~120  handle_device_messages
      download.rs     ~180  handle_download_messages
      model.rs        ~220  handle_model_messages
      backend.rs      ~240  handle_backend_messages + reload_if_active_backend + backend_model_provider
      settings.rs     ~170  handle_preview_typing_messages + handle_recording_stop_mode_messages
                            + handle_write_method_messages + custom_models_dir arms
      recording.rs    ~160  NEW handle_recording_messages (recording/audio/widget arms from update tail)
      shell.rs        ~60   NEW handle_shell_messages (OpenRepositoryUrl/ToggleContextPage/LaunchUrl)
      models_page.rs  ~420  handle_models_page_messages, split into 2 private sub-fns
```

> **Why delegation:** a `impl Trait for Type` block cannot be split across files, but inherent `impl Type` blocks can. So the `cosmic::Application` impl stays whole in `mod.rs`; its `init` body moves to `init.rs` as `fn init_model(...)` and the trait's `init` becomes a one-line call; same for `update` → `dispatch`. The short trait methods (`view`, `context_drawer`, `header_*`, `subscription`, `nav_model`, `on_nav_select`, `core`, `core_mut`) keep their bodies inline in `mod.rs` if that keeps `mod.rs` ≤ ~500; otherwise their bodies move to `view.rs`/`subscription.rs` as inherent helpers called from the trait method.

### Task B1: Create the directory module skeleton

**Files:**
- Create: `super-stt-app/src/core/app/mod.rs`
- Delete: `super-stt-app/src/core/app.rs` (its content moves into `app/mod.rs` and children across the next tasks)

- [ ] **Step 1: Move the file into the directory**

```bash
cd /home/jorge/rust_projects/super-stt/super-stt-app/src/core
mkdir app
git mv app.rs app/mod.rs
```

- [ ] **Step 2: Add child module declarations to `app/mod.rs`**

Near the top of `app/mod.rs`, after the existing `use` block, add:
```rust
mod events;
mod handlers;
mod init;
mod small_state;
mod subscription;
mod update;
mod view;
```
(Files referenced don't exist yet — they're created in B2–B9. Do not build until B2 creates them, or temporarily comment out declarations as you go.)

- [ ] **Step 3: Verify the move compiles before splitting**

Run: `cargo build -p super-stt-app` with the new `mod` lines commented out.
Expected: success (identical content, just relocated).

### Task B2: Extract free fns + tests → `app/events.rs`

**Files:**
- Create: `super-stt-app/src/core/app/events.rs`
- Modify: `super-stt-app/src/core/app/mod.rs`

- [ ] **Step 1: Move the free fns and test module**

Cut from `mod.rs`: `classify_daemon_error`, `settings_widget_event_to_message`, `widget_event_to_notification`, `raw_level_to_db_display_percent`, and the entire `#[cfg(test)] mod tests` block. Paste into `events.rs`. Add `// SPDX-License-Identifier: GPL-3.0-only` header. Add a `use` block for the types these reference (e.g. `use super::AppModel;` if needed, `crate::state::DaemonStatus`, `crate::ui::messages::Message`, shared types). Make the fns `pub(super)` if `mod.rs`/handlers call them (e.g. `classify_daemon_error` is called by handlers).

- [ ] **Step 2: Uncomment `mod events;` and build**

Run: `cargo build -p super-stt-app && cargo test -p super-stt-app --bins`
Expected: builds; the moved tests still pass.

### Task B3: Extract small state mutators → `app/small_state.rs`

**Files:**
- Create: `super-stt-app/src/core/app/small_state.rs`
- Modify: `super-stt-app/src/core/app/mod.rs`

- [ ] **Step 1: Move methods**

Cut from `mod.rs` the inherent methods `is_model_ready`, `set_model_downloading`, `set_model_loading`, `set_device_switching`, `update_title` into a new `impl AppModel { ... }` block in `small_state.rs`. Add SPDX header + `use super::*;` (or explicit imports). Keep their existing visibility (`pub` stays `pub`).

- [ ] **Step 2: Build**

Run: `cargo build -p super-stt-app`
Expected: success.

### Task B4: Extract `init` → `app/init.rs`

**Files:**
- Create: `super-stt-app/src/core/app/init.rs`
- Modify: `super-stt-app/src/core/app/mod.rs`

- [ ] **Step 1: Move the body**

In `init.rs`, create `impl AppModel { pub(super) fn init_model(core: cosmic::Core, flags: Self::Flags) -> (Self, Task<cosmic::Action<Message>>) { <original init body> } }`. Note `Self::Flags` is `()` — use the concrete type `()` since we're in an inherent impl, not the trait. Inspect the original `init` signature (L202) for exact param/return types and copy them concretely.

- [ ] **Step 2: Make the trait method delegate**

In `mod.rs`, replace the `init` body in `impl cosmic::Application` with:
```rust
fn init(core: cosmic::Core, _flags: Self::Flags) -> (Self, Task<cosmic::Action<Self::Message>>) {
    Self::init_model(core, _flags)
}
```
(Match the real signature; `_flags` name per original.)

- [ ] **Step 3: Build**

Run: `cargo build -p super-stt-app`
Expected: success.

### Task B5: Extract view methods → `app/view.rs`

**Files:**
- Create: `super-stt-app/src/core/app/view.rs`
- Modify: `super-stt-app/src/core/app/mod.rs`

- [ ] **Step 1: Decide inline vs extract**

Measure `mod.rs` after B2–B4. If `mod.rs` is comfortably ≤ 500 with `view`/`context_drawer`/`header_*` bodies inline, leave them. If not, move their bodies to inherent helpers in `view.rs` (`fn view_impl(&self)`, `fn context_drawer_impl(&self)`, etc.) and delegate from the trait methods, same pattern as B4.

- [ ] **Step 2: Build**

Run: `cargo build -p super-stt-app`
Expected: success.

### Task B6: Extract `subscription` → `app/subscription.rs`

**Files:**
- Create: `super-stt-app/src/core/app/subscription.rs`
- Modify: `super-stt-app/src/core/app/mod.rs`

- [ ] **Step 1: Move the free fn and (if extracted) the helper**

Move `audio_events_subscription` (free fn, was at L2492) into `subscription.rs`. If the trait `subscription` body is non-trivial, move it to an inherent `fn subscription_impl(&self)` here and delegate. Add SPDX + imports.

- [ ] **Step 2: Build**

Run: `cargo build -p super-stt-app`
Expected: success.

### Task B7: Create `handlers/` and move the simple handlers

**Files:**
- Create: `super-stt-app/src/core/app/handlers/mod.rs`
- Create: `super-stt-app/src/core/app/handlers/device.rs`, `download.rs`, `model.rs`, `backend.rs`, `settings.rs`
- Modify: `super-stt-app/src/core/app/mod.rs`

- [ ] **Step 1: Write `handlers/mod.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-only
mod backend;
mod daemon;
mod device;
mod download;
mod model;
mod models_page;
mod recording;
mod settings;
mod shell;
```
(Files for daemon/models_page/recording/shell are created in B8/B9 — comment those lines until then.)

- [ ] **Step 2: In `mod.rs`, declare `mod handlers;`** (already added in B1).

- [ ] **Step 3: Move each handler into its file**

Each file gets SPDX header + `use super::super::AppModel;` (the struct is in `core::app`, handlers are in `core::app::handlers`, so `super::super::AppModel` or `use crate::core::app::AppModel;`) + imports for `Message`, client fns, etc. Move:
- `device.rs`: `handle_device_messages`
- `download.rs`: `handle_download_messages`
- `model.rs`: `handle_model_messages`
- `backend.rs`: `handle_backend_messages`, `reload_if_active_backend`, `backend_model_provider`
- `settings.rs`: `handle_preview_typing_messages`, `handle_recording_stop_mode_messages`, `handle_write_method_messages`

Each method stays an inherent `impl AppModel { ... }` method with its original visibility (these are called from `dispatch`, so `pub(super)` or `pub(crate)` is sufficient — they're currently private; keep them callable from `update.rs`). Use `pub(in crate::core::app)` if a tighter scope is wanted, but `pub(super)` from a handler file resolves to `core::app::handlers`, which is NOT visible to `core::app::update`. **Use `pub(crate)` or `pub(in crate::core::app)`** so `update.rs`/`mod.rs` can call them.

- [ ] **Step 4: Build after each move**

Run: `cargo build -p super-stt-app`
Expected: success after each handler is moved and imports fixed.

### Task B8: Move + split `handle_daemon_messages` → `handlers/daemon.rs`

**Files:**
- Create: `super-stt-app/src/core/app/handlers/daemon.rs`
- Modify: `super-stt-app/src/core/app/handlers/mod.rs` (uncomment `mod daemon;`)

- [ ] **Step 1: Move the method**

Move `handle_daemon_messages` (was L900–1359, ~460 lines) into `daemon.rs` as `impl AppModel { pub(in crate::core::app) fn handle_daemon_messages(&mut self, message: Message) -> Task<cosmic::Action<Message>> { ... } }`.

- [ ] **Step 2: Split into sub-fns to clear `too_many_lines`**

The body is a `match message { ... }`. Group arms into private helper methods by domain and have `handle_daemon_messages` delegate:
```rust
pub(in crate::core::app) fn handle_daemon_messages(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
    match message {
        Message::ConnectToDaemon | Message::DaemonConnectionResult(_) | Message::DaemonConnected
        | Message::RetryConnection | Message::RetryAuthorization | Message::RefreshDaemonStatus
        | Message::PingTimeout | Message::DaemonError(_) | Message::WidgetBlocked(_) => {
            self.handle_daemon_connection(message)
        }
        Message::DaemonEventsReceived(_) | Message::DaemonEventsError(_) => {
            self.handle_daemon_events(message)
        }
        Message::CurrentAudioThemeLoaded(_) | Message::VolumeLoaded(_)
        | Message::CustomModelsDirLoaded(_) => self.handle_daemon_initial_state(message),
        _ => Task::none(),
    }
}
```
Create `handle_daemon_connection`, `handle_daemon_events`, `handle_daemon_initial_state` as private methods, each holding the matching arms from the original. Adjust the exact grouping to whatever keeps each sub-fn ≤ ~120 lines and under the `too_many_lines` threshold. **Verify the message-variant set exactly matches the original dispatch guard at L524–540** (no message dropped, none added).

- [ ] **Step 3: Build + test**

Run: `cargo build -p super-stt-app && cargo test -p super-stt-app --bins`
Expected: success.

### Task B9: Move the inline-tail handlers + split `handle_models_page_messages`

**Files:**
- Create: `super-stt-app/src/core/app/handlers/recording.rs`, `shell.rs`, `models_page.rs`
- Modify: `super-stt-app/src/core/app/handlers/settings.rs`, `super-stt-app/src/core/app/handlers/mod.rs`

- [ ] **Step 1: Extract the `update()` inline tail into handler methods**

From the original `update` body (the `match &message`/`match message` tail, L651–846), create:
- `recording.rs`: `pub(in crate::core::app) fn handle_recording_messages(&mut self, message: Message) -> Task<...>` covering `StartRecording`, `StopRecording`, `PreviewTextReceived`, `TranscriptionReceived`, `AudioLevelUpdate`, `AudioFeedbackToggled`, `AudioThemeSelected`, `SetAudioTheme`, `AudioThemesLoaded`, `VolumeChanged`, `WidgetAudioLevel`, `WidgetRecordingState`, `RecordingStateChanged`.
- `shell.rs`: `pub(in crate::core::app) fn handle_shell_messages(&mut self, message: Message) -> Task<...>` covering `OpenRepositoryUrl`, `ToggleContextPage`, `LaunchUrl`.
- `settings.rs` (append): fold the `CustomModelsDirInput`/`CustomModelsDirEdit`/`CustomModelsDirSet`/`CustomModelsDirError` arms into a `handle_custom_models_dir_messages` method (or extend the existing settings handler).

Copy arm bodies verbatim; only change the surrounding `match` scaffolding.

- [ ] **Step 2: Move + split `handle_models_page_messages` → `models_page.rs`**

Move `handle_models_page_messages` (was L1949–2422, ~470 lines). Split its `match` into 2 private sub-fns (e.g. `handle_models_page_ui` for tab/stage/select/config arms, `handle_models_page_registry` for install/registry/import arms), delegated from `handle_models_page_messages`. Keep each ≤ ~250 lines so `too_many_lines` is satisfied without an allow. Verify variant coverage matches L560–594 exactly.

- [ ] **Step 3: Build + test**

Run: `cargo build -p super-stt-app && cargo test -p super-stt-app --bins`
Expected: success.

### Task B10: Move `update` dispatcher → `app/update.rs`, finalise

**Files:**
- Create: `super-stt-app/src/core/app/update.rs`
- Modify: `super-stt-app/src/core/app/mod.rs`

- [ ] **Step 1: Move the dispatcher**

Move the `matches!`-routing portion of `update` (L522–649 + the now-relocated tail's delegation) into `update.rs` as `impl AppModel { pub(in crate::core::app) fn dispatch(&mut self, message: Message) -> Task<cosmic::Action<Message>> { ... } }`. The body becomes the chain of `matches!(...) { return self.handle_*(message); }` guards plus, at the end, a final `matches!` for recording/shell/custom-dir routing to the new handlers, then `Task::none()`. No inline arm bodies remain here.

- [ ] **Step 2: Trait `update` delegates**

In `mod.rs`:
```rust
fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
    self.dispatch(message)
}
```

- [ ] **Step 3: Remove all 7 `#[allow(clippy::too_many_lines)]`**

Run: `grep -rn 'too_many_lines' super-stt-app/src/`
Expected: no output. If clippy still flags a function, split it further (it's still too long).

- [ ] **Step 4: Full gate**

Run:
```bash
cargo clippy --all-features --workspace -- -W clippy::pedantic -D warnings -D unused_must_use
cargo test -p super-stt-app --bins
find super-stt-app/src/core -name '*.rs' -exec wc -l {} \; | sort -rn | head
```
Expected: clippy passes; tests pass; no `core/app/*` file over ~600 lines.

### Task B11: Commit Phase B

- [ ] **Step 1: Commit** (after user authorisation)

```bash
git add super-stt-app/src/core/
git commit -m "Split core/app.rs into app/ directory module with per-domain message handlers"
```

---

## Phase C — Split `ui/views/models.rs` (1 commit)

All free functions; plain moves into a `models/` directory module, organised by widget with reusable primitives lifted out.

### Target layout
```
ui/views/
  mod.rs                  change `pub mod models;` stays (now resolves to models/mod.rs)
  models/
    mod.rs          ~160  pub fn page() + tab dispatch + ModelStatus type + module decls
    status.rs       ~150  model_status, classify_model_status + their tests
    fmt.rs          ~140  fmt_gib, fmt_gib_pair, short_gpu_name, vram_meter, vram_warning
    surface.rs      ~180  card_surface, card_divider, bordered_scroll_view,
                          tab_bar_container, toolbar_container, muted_text_color
    chips.rs        ~280  capability_chip, capability_chips, count_chip, cloud_chip,
                          chip_group, result_count, requirement_warning,
                          backend_is_online, backend_supports_gpu, backend_supports_cpu
    tabs.rs         ~120  models_tab_switcher, tab_inner_class
    active.rs       ~400  active_backend_card, staged_model_picker, loaded_model_summary,
                          backend_header, backend_glyph_tile, vram_shortfall,
                          staged_vram_shortfall, unmet_requirements + tests
    installed.rs    ~360  installed_tab, installed_card, installed_overflow_menu,
                          update_available, card_download_progress, card_error + tests
    download.rs     ~360  download_split, download_toolbar, download_empty_state,
                          download_card, phase_label, models_line, rounded_tooltip
    add_sheet.rs    ~150  add_backend_sheet, registry_entry_matches
    configure.rs    ~300  configure_sheet, config_label, secret_row, option_row + tests
```

> Several fns currently `fn foo(...)` are file-private. After moving, any fn used by a sibling module must become `pub(super)` (visible within `models/`). External callers only reach `page`, `add_backend_sheet`, `configure_sheet` — those stay `pub`.

### Task C1: Create directory + move file

**Files:**
- Create: `super-stt-app/src/ui/views/models/mod.rs`
- Delete: `super-stt-app/src/ui/views/models.rs`

- [ ] **Step 1:**
```bash
cd /home/jorge/rust_projects/super-stt/super-stt-app/src/ui/views
mkdir models
git mv models.rs models/mod.rs
```

- [ ] **Step 2: Build** — `cargo build -p super-stt-app`. Expected: success (relocated, unchanged).

### Task C2: Extract primitives — `fmt.rs`, `surface.rs`, `chips.rs`, `status.rs`

**Files:**
- Create: `models/fmt.rs`, `models/surface.rs`, `models/chips.rs`, `models/status.rs`
- Modify: `models/mod.rs`

- [ ] **Step 1: Add module declarations to `mod.rs`**

```rust
mod chips;
mod fmt;
mod status;
mod surface;
```

- [ ] **Step 2: Move functions per the layout table**

Move the listed fns into each file with SPDX header + `use super::*;` (or explicit imports for `AppModel`, `Message`, cosmic widget types). Mark each moved fn `pub(super)` so callers in sibling modules resolve it via `super::fmt::fmt_gib`, etc. Update call sites in the remaining code to the new paths (or add `use super::fmt::*;` style re-imports at the top of consuming modules). The `#[allow(clippy::cast_precision_loss)]` on fmt fns and `#[allow(clippy::similar_names)]` on `capability_chips` travel with their functions (with the `// reason:` lines from Phase A).

- [ ] **Step 3: Build + test** — `cargo build -p super-stt-app && cargo test -p super-stt-app --bins`. Expected: success; moved tests pass.

### Task C3: Extract widget modules — `tabs.rs`, `active.rs`, `installed.rs`, `download.rs`, `add_sheet.rs`, `configure.rs`

**Files:**
- Create: `models/tabs.rs`, `models/active.rs`, `models/installed.rs`, `models/download.rs`, `models/add_sheet.rs`, `models/configure.rs`
- Modify: `models/mod.rs`

- [ ] **Step 1: Add module declarations to `mod.rs`**

```rust
mod active;
mod add_sheet;
mod configure;
mod download;
mod installed;
mod tabs;
```

- [ ] **Step 2: Move functions per the layout table, one module at a time**

For each module: cut the listed fns into the file, add SPDX + imports, set visibility (`pub(super)` for cross-module use; `pub` for `add_backend_sheet`/`configure_sheet` re-exported from `mod.rs`). After each module, run `cargo build -p super-stt-app` and fix import paths before moving to the next. Keep `#[cfg(test)] mod tests` blocks with the functions they exercise (active/installed/configure carry tests).

- [ ] **Step 3: Re-export public surface from `mod.rs`**

Ensure `mod.rs` still exposes the public API. If `page` stays in `mod.rs`, leave it. For `add_backend_sheet`/`configure_sheet`, add:
```rust
pub use add_sheet::add_backend_sheet;
pub use configure::configure_sheet;
```
Verify external callers (`core/app/view.rs`, handlers) still resolve these.

- [ ] **Step 4: Full gate**

Run:
```bash
cargo clippy --all-features --workspace -- -W clippy::pedantic -D warnings -D unused_must_use
cargo test -p super-stt-app --bins
find super-stt-app/src/ui/views/models -name '*.rs' -exec wc -l {} \; | sort -rn
```
Expected: clippy passes; tests pass; no `models/*` file over ~600 lines.

### Task C4: Commit Phase C

- [ ] **Step 1: Commit** (after user authorisation)

```bash
git add super-stt-app/src/ui/views/
git commit -m "Split ui/views/models.rs into models/ directory module by widget with shared primitives"
```

---

## Phase D — Standardise shared helpers (1 commit)

Cleanup pass. Extract **only** patterns that already repeat 3+ times in the now-split code. No speculative abstraction. The bar: the helper shortens its 3+ call sites and needs no new wrapper type/trait.

### Task D1: Audit for repetition

**Files:** none modified yet.

- [ ] **Step 1: Find the "set X, spawn Task, map result to Message" pattern**

Run:
```bash
grep -rn 'Task::perform' super-stt-app/src/core/app/handlers/
```
Inspect the results. The common shape is:
```rust
Task::perform(some_client_fn(arg), |result| match result {
    Ok(_) => cosmic::Action::App(Message::SomeOk),
    Err(e) => cosmic::Action::App(Message::SomeErr(e)),
})
```
Count distinct sites with this exact Ok/Err-to-Message shape.

- [ ] **Step 2: Decide**

If ≥ 3 sites share the shape with only the future and the two Message variants differing, design a helper in `core/app/handlers/mod.rs`:
```rust
pub(in crate::core::app) fn perform<F, T>(
    fut: F,
    on_ok: fn(T) -> Message,
    on_err: fn(String) -> Message,
) -> Task<cosmic::Action<Message>>
where
    F: std::future::Future<Output = Result<T, String>> + Send + 'static,
    T: Send + 'static,
{
    Task::perform(fut, move |r| match r {
        Ok(v) => cosmic::Action::App(on_ok(v)),
        Err(e) => cosmic::Action::App(on_err(e)),
    })
}
```
(Adjust the error type to the actual client error type — inspect a client fn signature first.) If sites are too bespoke (different Task::batch, stream, side effects), **do not** extract — record "no extraction warranted" and skip to D2.

- [ ] **Step 3: Apply the helper (if warranted)**

Replace the 3+ matching call sites with `Self::perform(...)`. Build after each: `cargo build -p super-stt-app`.

### Task D2: Confirm error-routing consistency

**Files:** possibly `core/app/handlers/*.rs`

- [ ] **Step 1: Find ad-hoc error→status mapping**

Run: `grep -rn 'classify_daemon_error\|DaemonStatus::' super-stt-app/src/core/app/`
Confirm every place that derives a `DaemonStatus` from an error string routes through `classify_daemon_error`. If a handler re-implements the same string matching inline, replace it with a `classify_daemon_error` call.

- [ ] **Step 2: Build** — `cargo build -p super-stt-app`. Expected: success.

### Task D3: Final verification + commit

- [ ] **Step 1: Full gate**

Run:
```bash
cargo clippy --all-features --workspace -- -W clippy::pedantic -D warnings -D unused_must_use
cargo test -p super-stt-app --bins
grep -rn 'allow(clippy::too_many_lines)' super-stt-app/src/   # expect: empty
grep -rn 'mockup' super-stt-app/src/                          # expect: empty
find super-stt-app/src -name '*.rs' -exec wc -l {} \; | sort -rn | head -15
```
Expected: clippy passes; tests pass; first two greps empty; no file meaningfully over ~600 lines (target ~500).

- [ ] **Step 2: Compare against baseline**

Run: `diff <(sort /tmp/baseline_loc.txt) <(find super-stt-app/src -name '*.rs' -exec wc -l {} \; | sort)` for a before/after sanity read (informational only).

- [ ] **Step 3: Commit** (after user authorisation)

```bash
git add super-stt-app/src/
git commit -m "Standardise task-dispatch helper and unify daemon error routing in settings app"
```

---

## Commit policy

Per the user's standing preference, **do not run `git commit` or `git push` without explicit per-action authorisation**, and keep commit messages high-level (no Co-Authored-By trailers, no verbose bullet lists). The commit steps above are gated on that authorisation. One commit per phase.

## Self-review notes (author)

- **Spec coverage:** Phase A covers the comment + allow-justification goals; B removes all `too_many_lines` and the app.rs size goal; C the models.rs size + by-widget/primitives goal; D the "standardise reusable functions" goal. Acceptance gates from the spec appear as the per-phase "Full gate" steps.
- **Private-field hazard** (handlers touch `self.core`/`self.context_page`/`self.nav`) is resolved by making handlers descendants of `core::app` (directory module) — see B1 and the visibility notes in B7 (`pub(in crate::core::app)`).
- **Trait-impl-can't-split hazard** resolved by delegation (B4, B10).
- **Behaviour-preservation:** every handler split step includes "verify variant coverage matches the original dispatch guard exactly" so no message is dropped or rerouted.
- **Deferred/out of scope:** `daemon/client.rs` (separate agent), `AppModel` field-count, daemon & shared crates.
