# Settings app code-quality refactor

**Status:** Draft
**Date:** 2026-06-01
**Scope:** `super-stt-app/` only

## Problem

Two source files dominate `super-stt-app/src/` and make the crate hard to navigate, review, and reason about:

| File | Lines | Why it grew |
|---|---|---|
| `core/app.rs` | 2791 | `cosmic::Application` impl + 8 `handle_*` message-routing methods, several over the clippy 100-line default and silenced with `#[allow(clippy::too_many_lines)]`. |
| `ui/views/models.rs` | 2451 | ~50 view-building functions for the Models page across multiple tabs and sheets. |

Secondary issues across the crate:

- 14 `#[allow(clippy::...)]` annotations, 7 of which are `too_many_lines`.
- ~17 comments in `ui/views/models.rs` that narrate a private design mockup ("matches the mockup's `.fgroup`", etc.). These are AI-style decoration: they document a no-longer-relevant external artefact, not a non-obvious invariant.
- A handful of narration comments elsewhere (`For now`, `Note that we ...`) that explain *what* code does rather than *why*.

The 757-line `daemon/client.rs` is on the cusp but explicitly out of scope — a separate effort will handle daemon-adjacent refactors.

## Goals

- No file in `super-stt-app/src/` over ~500 lines (soft cap 600). Soft, not hard — going slightly over is acceptable if a split would harm cohesion.
- Zero `#[allow(clippy::too_many_lines)]` in `super-stt-app/`.
- Remaining `#[allow(clippy::...)]` annotations each carry a one-line `// reason: ...` justification.
- Zero comments referencing the design mockup; zero AI-narration comments that don't document a hidden invariant.
- `cargo clippy --all-features --workspace -- -W clippy::pedantic -D warnings -D unused_must_use` passes throughout.
- `cargo test -p super-stt-app` passes throughout.
- No behavioural changes. This is a pure refactor.

## Non-goals

- Refactoring `super-stt-daemon`, `super-stt-shared`, `super-stt-cli`, `super-stt-cosmic-applet`, or `super-stt-consent`.
- Refactoring `super-stt-app/src/daemon/client.rs` (separate agent).
- Removing legitimate `#[allow]`s that document a real lint exception (e.g., `cast_precision_loss` on display-only f32 conversions).
- Splitting `AppModel` into sub-structs. `struct_excessive_bools` is the shape COSMIC pushes; we leave it with a justification.
- Introducing new abstractions or design patterns. We are de-duplicating proven repetition, not designing future flexibility.

## Approach

Five phases, **one commit per phase**, in order. Each phase ends with the clippy + test gates green.

### Phase A — Comment and allow audit

Touches comments and `#[allow]` lines only. No structural changes.

1. **Mockup references.** Delete the ~17 mockup comments in `ui/views/models.rs`. Each comment is reviewed individually with the rule: keep only if removing it would hide a non-obvious invariant the code itself cannot express. Concretely:
   - `"matches the mockup's .fgroup"` → delete (decorative cross-reference).
   - `"footer action row (matches the mockup's card divider)"` → delete.
   - `"A leading play glyph fronts the label, like the mockup's '▶ Load model'"` → delete.
2. **Other narration.** Sweep for `For now`, `Note that we`, `We don't ...`, similar narrators across all of `super-stt-app/src/`. Same rule.
3. **Clippy allows.** For each remaining `#[allow(clippy::...)]`:
   - If it can be removed by a small inline rewrite (e.g. an extracted helper for a cast), do it.
   - Otherwise add or refine a `// reason: ...` justification on the line above.
4. Do **not** remove `#[allow(clippy::too_many_lines)]` in this phase; those go away naturally in Phases B and C.

**Baseline capture:** Before edits, record per-file LOC and the full `cargo clippy` output, save to `/tmp/baseline.txt` for diff comparison. (Not committed — discarded after Phase E.)

### Phase B — Split `core/app.rs`

Goal: every file ≤ ~500 lines; every function passes `too_many_lines` without allow.

Target layout under `super-stt-app/src/core/`:

```
core/
  mod.rs                  ~30   re-exports for cross-crate callers
  app.rs                  ~400  AppModel struct + cosmic::Application impl shell
  app_init.rs             ~200  fn init() body, factored out
  app_view.rs             ~200  header_start/header_end/view/context_drawer/nav_model
  app_subscription.rs     ~250  subscription() + audio_events_subscription()
  app_update.rs           ~250  fn update() — the giant match, dispatcher only
  events.rs               ~250  classify_daemon_error, settings_widget_event_to_message,
                                widget_event_to_notification, raw_level_to_db_display_percent,
                                MenuAction impl, and their tests
  handlers/
    mod.rs                ~30   pub use re-exports
    daemon.rs             ~400  handle_daemon_messages, split into 2-3 sub-fns grouped by
                                related message variants (status / recording / model-events)
    device.rs             ~150  handle_device_messages
    download.rs           ~200  handle_download_messages
    model.rs              ~250  handle_model_messages
    backend.rs            ~250  handle_backend_messages + reload_if_active_backend
                                + backend_model_provider
    settings.rs           ~150  handle_preview_typing_messages,
                                handle_recording_stop_mode_messages,
                                handle_write_method_messages (small + closely related)
    models_page.rs        ~400  handle_models_page_messages, split into ~2 sub-fns
```

**Splitting heuristic for still-long functions:** a `match` over the message enum with N arms can be split into multiple sub-fns whose arms are grouped by domain (e.g., "recording state updates" vs. "model events" vs. "device probe results"). Each sub-fn returns `Task<cosmic::Action<Message>>`. The parent function becomes a flat dispatcher.

**`update()` itself stays as a dispatcher**: the body is a `match message { ... }` whose arms call the appropriate `handle_*_messages` method. Even with 8+ handler categories this fits in a single ~250-line file.

**Visibility:** handler methods stay as `impl AppModel { fn handle_*(...) }` blocks declared in their respective files. This requires each handler file to start with `impl AppModel { ... }` — Rust permits multiple `impl` blocks for the same type across modules of the same crate, so this is well-formed.

### Phase C — Split `ui/views/models.rs`

Goal: by-widget organisation with shared primitives extracted to their own modules.

Target layout under `super-stt-app/src/ui/views/models/`:

```
mod.rs              ~150  pub fn page() + tab dispatch + ModelStatus enum
status.rs           ~150  model_status, classify_model_status + tests
fmt.rs              ~120  fmt_gib, fmt_gib_pair, short_gpu_name, vram_meter, vram_warning
                          (primitives — formatting + small visual indicators)
surface.rs          ~180  card_surface, card_divider, bordered_scroll_view,
                          tab_bar_container, toolbar_container, muted_text_color
                          (primitives — container chrome)
chips.rs            ~280  capability_chip, capability_chips, count_chip, cloud_chip,
                          chip_group, result_count, requirement_warning
                          (primitives — chip family)
tabs.rs             ~120  models_tab_switcher, tab_inner_class
                          (Models-page-specific tab bar)
active.rs           ~400  active_backend_card, staged_model_picker, loaded_model_summary,
                          backend_header, backend_glyph_tile, vram_shortfall,
                          staged_vram_shortfall, unmet_requirements + tests
                          (widget: the "Active" tab)
installed.rs        ~350  installed_tab, installed_card, installed_overflow_menu,
                          update_available, card_download_progress, card_error + tests
                          (widget: the "Installed" tab)
download.rs         ~350  download_split, download_toolbar, download_empty_state,
                          download_card, phase_label, models_line, rounded_tooltip
                          (widget: the "Download" tab)
add_sheet.rs        ~150  add_backend_sheet, registry_entry_matches
                          (widget: the Browse-registry sheet)
configure.rs        ~300  configure_sheet, config_label, secret_row, option_row + tests
                          (widget: the per-backend Configure sheet)
```

**Primitives vs widgets distinction:** `fmt.rs`, `surface.rs`, `chips.rs` hold reusable building blocks — anything that could plausibly be called from elsewhere in `ui/views/` later. The widget modules (`active.rs`, `installed.rs`, `download.rs`, `add_sheet.rs`, `configure.rs`) hold composite views that own a visual area on screen.

**Test placement:** existing `#[cfg(test)] mod` blocks travel with the function they exercise.

**Public API of the `models` module is unchanged.** `pub fn page()` and `pub fn add_backend_sheet()` and `pub fn configure_sheet()` continue to be the only items external callers reach.

### Phase D — Standardise shared helpers

Cleanup pass after the structural moves. Only extract patterns that **already repeat 3+ times** in the now-visible code. Speculative abstractions are out of scope.

Candidates to investigate (not commitments):

1. **The "set X, spawn Task that calls client fn, map result to Message" pattern.** Every setter handler currently does this by hand. If it repeats verbatim across 3+ sites with only the client fn and the variant differing, extract a small generic helper. If each site is meaningfully bespoke, leave them alone.
2. **Error → `DaemonStatus` conversion.** `classify_daemon_error` already exists; confirm every handler routes through it and there's no parallel ad-hoc string matching elsewhere.
3. **Notification / toast construction.** If multiple handlers build a `widget_event_to_notification`-style call by hand, expose a single helper.

The bar for committing an extraction: the new helper must shorten its 3+ call sites and not require a wrapping type or trait to do so.

### Phase E — Verification

Final gate, before the phase commit lands:

- `cargo build --workspace`
- `cargo clippy --all-features --workspace -- -W clippy::pedantic -D warnings -D unused_must_use`
- `cargo test -p super-stt-app --lib` (HTTP integration tests are excluded — see memory `[[reference_daemon_tests_need_keyring]]`)
- `wc -l super-stt-app/src/**/*.rs | sort -rn | head` to confirm no file over ~600 lines
- `grep -rn "allow(clippy::too_many_lines)" super-stt-app/src/` returns nothing
- `grep -rn "mockup" super-stt-app/src/` returns nothing

If any check fails, the failure goes back to the relevant phase, not a sixth commit.

## Risk and rollback

- **Risk:** module re-organisation breaks an import path used by `super-stt-cli` or another sibling crate. Mitigation: `cargo build --workspace` is part of every phase gate.
- **Risk:** splitting `handle_daemon_messages` introduces a behaviour change by re-ordering effects. Mitigation: each sub-fn handles a disjoint subset of message variants; no state is mutated between sub-fn dispatch and return.
- **Risk:** a handler's `Task<Action<Message>>` return depends on mutable borrows of `self` that don't compose cleanly once split into sibling-module functions. Mitigation: keep all handler methods on `impl AppModel`, not free functions; mut-borrow scope stays inside each method.
- **Rollback:** each phase is one commit. `git revert` the offending commit; subsequent phases are independent and can be redone on the reverted base.

## Out of scope (logged for follow-up)

- `super-stt-daemon/src/daemon/http_server.rs` (3655 lines) — separate agent.
- `super-stt-shared/src/daemon/http_client.rs` (1537), `super-stt-shared/src/models/protocol.rs` (1420) — separate effort.
- `super-stt-app/src/daemon/client.rs` (757 lines) — separate agent.
- `AppModel` field-count reduction — out of scope; COSMIC apps land here naturally.
