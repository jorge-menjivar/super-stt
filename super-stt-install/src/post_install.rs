// SPDX-License-Identifier: GPL-3.0-only
//! Post-install steps run back in the unprivileged user process after the
//! root phase has placed files: systemd service (re)start, applet panel
//! restart, launcher-cache nudge, legacy `~/.local` cleanup, and the COSMIC
//! keyboard-shortcut migrate/add. A faithful port of `scripts/install-beta.sh`'s
//! equivalent steps. Every step is best-effort (`log::warn!` on failure)
//! except the daemon restart, the one failure the app must hear about.
//!
//! `run`'s WHAT-to-do is a pure decision, separated from the HOW: [`plan`]
//! takes the coarse facts already known (or cheap to check) before any file
//! I/O or process spawn and returns an ordered [`Step`] list with no I/O of
//! its own; `run` computes those inputs, calls `plan`, and executes the
//! result through a thin per-step match. That seam is the whole decision
//! tree's test coverage — see `plan`'s unit tests below — without ever
//! needing a command-runner trait or a mocked OS.

use std::path::Path;

use crate::errors::InstallError;
use crate::stage::Components;

/// Run `bin` with `args`, discarding output, returning whether it exited 0.
/// A spawn failure (binary missing, etc.) also counts as "not ok".
async fn cmd_ok(bin: &str, args: &[&str]) -> bool {
    tokio::process::Command::new(bin)
        .args(args)
        .status()
        .await
        .is_ok_and(|s| s.success())
}

/// Whether `bin` resolves on the current `$PATH`.
fn on_path(bin: &str) -> bool {
    let path_env = std::env::var("PATH").unwrap_or_default();
    crate::escalate::which(bin, &path_env).is_some()
}

/// One post-install action, in the order [`plan`] returns them. Each variant
/// is a single, independent shell-out or filesystem tweak — see `run`'s
/// executor `match` for what each one actually does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// `systemctl --user daemon-reload`.
    DaemonReload,
    /// Remove a legacy `~/.config/systemd/user/super-stt.service` unit that
    /// would otherwise shadow the packaged one and keep launching the (now
    /// deleted) legacy `~/.local/bin` binary.
    RemoveLegacyUnit,
    /// `systemctl --user enable super-stt`.
    Enable,
    /// `systemctl --user restart|start super-stt` (verb picked at execution
    /// time, via `is-active`) — the one step whose failure is a hard error.
    RestartOrStart,
    /// Restart `cosmic-panel` so it picks up the just-updated applet binary.
    RestartPanel,
    /// Nudge COSMIC's launcher caches (app grid + search backend) to rescan
    /// desktop entries.
    NudgeLaunchers,
    /// Remove pre-`/usr/local` per-user leftovers (bins/desktop files/icons).
    CleanupLegacy,
    /// Rewrite a legacy `~/.local/bin/stt` shortcut reference, if present.
    MigrateShortcut,
    /// Interactively offer to add the `Super+Space` shortcut.
    PromptShortcut,
}

/// Decide which [`Step`]s to run, and in what order, from facts already known
/// before any post-install I/O: `components` (what was just installed or
/// updated), `applet_was_installed` (captured *before* the root phase ran —
/// the script's `is_update` check), `interactive`, and three environment
/// probes the caller (`run`) is expected to have already made:
/// `systemctl_available`/`cosmic_available` (`$PATH` lookups) and
/// `panel_running` (a `pgrep` check). `panel_running` only matters when
/// `components.applet && applet_was_installed` — a caller may cheaply pass
/// `true` unconditionally for any other combination and it's simply ignored.
///
/// Order mirrors `scripts/install-beta.sh`: systemd install/enable/restart,
/// then the applet's panel restart, then the launcher-cache nudge, then
/// legacy cleanup, then the COSMIC shortcut. The script interleaves each
/// component's legacy-file cleanup with that component's own binary
/// placement; here all placement already happened earlier, in the root
/// phase, so [`Step::CleanupLegacy`] runs once, unconditionally, instead of
/// once per component — removing files that were never there to begin with
/// is a no-op, so this changes only *when* the (idempotent) sweep happens,
/// never what ends up on disk.
#[must_use]
#[allow(clippy::fn_params_excessive_bools)] // interface fixed by the design doc: the five booleans are the planner's whole point
pub fn plan(
    components: Components,
    applet_was_installed: bool,
    interactive: bool,
    systemctl_available: bool,
    cosmic_available: bool,
    panel_running: bool,
) -> Vec<Step> {
    let mut steps = Vec::new();

    if components.daemon && systemctl_available {
        steps.push(Step::DaemonReload);
        steps.push(Step::RemoveLegacyUnit);
        steps.push(Step::Enable);
        steps.push(Step::RestartOrStart);
    }

    if components.applet && applet_was_installed && panel_running {
        steps.push(Step::RestartPanel);
    }

    // The script skips the launcher nudge only for `INSTALL_OPTION=daemon`
    // (daemon-only): an app or applet install both add launcher-visible
    // entries that benefit from the rescan.
    let daemon_only = components.daemon && !components.app && !components.applet;
    if !daemon_only {
        steps.push(Step::NudgeLaunchers);
    }

    steps.push(Step::CleanupLegacy);

    if components.daemon && cosmic_available {
        steps.push(Step::MigrateShortcut);
        if interactive {
            steps.push(Step::PromptShortcut);
        }
    }

    steps
}

async fn step_daemon_reload() {
    if !cmd_ok("systemctl", &["--user", "daemon-reload"]).await {
        log::warn!("systemctl --user daemon-reload failed");
    }
}

/// A unit left in `~/.config/systemd/user` by an older install takes
/// precedence over the packaged one — remove it or systemd keeps launching
/// the (now deleted) legacy `~/.local/bin` binary.
fn step_remove_legacy_unit() {
    if let Some(home) = dirs::home_dir() {
        let legacy_unit = home.join(".config/systemd/user/super-stt.service");
        if legacy_unit.exists() {
            let _ = std::fs::remove_file(&legacy_unit);
        }
    }
}

async fn step_enable() {
    if !cmd_ok("systemctl", &["--user", "enable", "super-stt"]).await {
        log::warn!("systemctl --user enable super-stt failed");
    }
}

/// # Errors
/// [`InstallError::PostInstallFailed`] when the final `restart`/`start`
/// exits nonzero — the only step in the whole post-install sequence whose
/// failure is a hard error.
async fn step_restart_or_start() -> Result<(), InstallError> {
    let is_active = cmd_ok(
        "systemctl",
        &["--user", "is-active", "--quiet", "super-stt"],
    )
    .await;
    let verb = if is_active { "restart" } else { "start" };
    if cmd_ok("systemctl", &["--user", verb, "super-stt"]).await {
        Ok(())
    } else {
        Err(InstallError::PostInstallFailed(format!(
            "daemon {verb} failed: run `systemctl --user {verb} super-stt` manually"
        )))
    }
}

async fn step_restart_panel() {
    if !cmd_ok("pkill", &["-f", "cosmic-panel"]).await {
        log::warn!("failed to restart cosmic-panel to load the updated applet");
    }
}

/// Nudge COSMIC's launcher caches (app grid + search backend): both scan
/// desktop entries at session start and miss entries added to a directory
/// they weren't watching. They respawn on demand and rescan.
async fn step_nudge_launchers() {
    let _ = tokio::process::Command::new("pkill")
        .args(["-f", "^cosmic-app-library$"])
        .status()
        .await;
    let _ = tokio::process::Command::new("pkill")
        .args(["-f", "^cosmic-launcher$"])
        .status()
        .await;
    let _ = tokio::process::Command::new("pkill")
        .args(["-f", "^pop-launcher( |$)"])
        .status()
        .await;
}

/// Bins, desktop files, and icons the pre-`/usr/local` per-user install left
/// behind — cleared so they don't shadow (or duplicate in launchers) the
/// fresh install. Exact lists from `scripts/install-beta.sh`.
fn cleanup_legacy() {
    let Some(home) = dirs::home_dir() else {
        return;
    };

    let bin_dir = home.join(".local/bin");
    for name in [
        "super-stt",
        "super-stt-daemon",
        "super-stt-cli",
        "super-stt-consent",
        "stt",
        "super-stt-app",
        "super-stt-cosmic-applet",
        "super-stt-applet-full",
        "super-stt-applet-left",
        "super-stt-applet-right",
    ] {
        let _ = std::fs::remove_file(bin_dir.join(name));
    }

    let apps_dir = home.join(".local/share/applications");
    for name in [
        "super-stt-app.desktop",
        "super-stt-cosmic-applet-full.desktop",
        "super-stt-cosmic-applet-left.desktop",
        "super-stt-cosmic-applet-right.desktop",
    ] {
        let _ = std::fs::remove_file(apps_dir.join(name));
    }

    let icons_dir = home.join(".local/share/icons");
    let _ = std::fs::remove_file(icons_dir.join("super-stt-app.svg"));
    let _ = std::fs::remove_file(icons_dir.join("hicolor/scalable/apps/super-stt-app.svg"));
    let _ =
        std::fs::remove_file(icons_dir.join("hicolor/scalable/apps/super-stt-cosmic-applet.svg"));

    let _ = std::fs::remove_file(home.join(".local/share/metainfo/super-stt-app.metainfo.xml"));
}

/// Rewrite a legacy `<home>/.local/bin/stt ` shortcut command to
/// `<prefix>/bin/stt `, since the wrapper no longer lives in the removed
/// per-user layout. Returns `None` when `content` doesn't reference the
/// legacy path (nothing to migrate).
#[must_use]
pub fn migrate_shortcut_content(content: &str, home: &str, prefix: &str) -> Option<String> {
    let legacy = format!("{home}/.local/bin/stt ");
    if !content.contains(&legacy) {
        return None;
    }
    let replacement = format!("{prefix}/bin/stt ");
    Some(content.replace(&legacy, &replacement))
}

/// Build the `Spawn(...)` shortcut entry block for `stt_command`, RON-shaped
/// like the rest of the COSMIC shortcuts file.
fn shortcut_entry(stt_command: &str) -> String {
    format!(
        "    (\n        modifiers: [\n            Super,\n        ],\n        key: \"space\",\n        description: Some(\"Super STT\"),\n    ): Spawn(\"{stt_command}\"),\n"
    )
}

/// Add a `Super+Space` → `stt_command` shortcut entry to `content` (the
/// COSMIC custom-shortcuts file's current text), mirroring
/// `scripts/install-beta.sh:277-377`'s coarse checks: skip (return `None`)
/// if a "Super STT" entry already exists, or if `key: "space"` is already
/// bound to a `Super`-modified shortcut (approximated, like the script's
/// grep, as both substrings appearing anywhere in `content`). Empty or
/// `{}`-only content gets the full-file template; otherwise the entry is
/// inserted before the final closing brace.
#[must_use]
pub fn shortcut_with_super_stt(content: &str, stt_command: &str) -> Option<String> {
    if content.contains("Super STT") {
        return None;
    }
    if content.contains("key: \"space\"") && content.contains("Super") {
        return None;
    }

    let entry = shortcut_entry(stt_command);
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return Some(format!("{{\n{entry}}}\n"));
    }

    // File has content: drop everything from (and including) the final `}`
    // and append our entry plus a fresh close — mirrors the script's
    // `head -n -1` + heredoc.
    let last_brace = content.rfind('}')?;
    let head = &content[..last_brace];
    Some(format!("{head}{entry}}}\n"))
}

/// Read `/dev/tty` for a `[Y/n]`-style answer to `prompt` (echoed to
/// stderr first). Defaults to yes on anything but an exact `n`/`N` — same
/// as the script's `[[ "$add_shortcut" =~ ^[Nn]$ ]]` check. A `/dev/tty`
/// open/read failure also defaults to "no" (never silently proceeds without
/// having actually asked).
fn prompt_yes_no(prompt: &str) -> bool {
    use std::io::{BufRead, Write};
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let Ok(tty) = std::fs::File::open("/dev/tty") else {
        return false;
    };
    let mut line = String::new();
    if std::io::BufReader::new(tty).read_line(&mut line).is_err() {
        return false;
    }
    !line.trim().eq_ignore_ascii_case("n")
}

/// Rewrite a legacy `~/.local/bin/stt` shortcut reference in the COSMIC
/// custom-shortcuts file, if present.
fn migrate_shortcut(prefix: &Path) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let shortcuts_file =
        home.join(".config/cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom");
    if let Ok(content) = std::fs::read_to_string(&shortcuts_file)
        && let Some(migrated) =
            migrate_shortcut_content(&content, &home.to_string_lossy(), &prefix.to_string_lossy())
        && let Err(e) = std::fs::write(&shortcuts_file, migrated)
    {
        log::warn!("failed to migrate COSMIC shortcut: {e}");
    }
}

/// Interactively offer to add the `Super+Space` shortcut.
fn prompt_shortcut(prefix: &Path) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let shortcuts_dir = home.join(".config/cosmic/com.system76.CosmicSettings.Shortcuts/v1");
    let shortcuts_file = shortcuts_dir.join("custom");

    if !prompt_yes_no("Add COSMIC keyboard shortcut (Super+Space)? [Y/n]: ") {
        return;
    }

    if let Err(e) = std::fs::create_dir_all(&shortcuts_dir) {
        log::warn!("failed to create COSMIC shortcuts dir: {e}");
        return;
    }
    let stt_command = format!("{}/bin/stt record --write", prefix.display());
    let existing = std::fs::read_to_string(&shortcuts_file).unwrap_or_default();
    if let Some(updated) = shortcut_with_super_stt(&existing, &stt_command)
        && let Err(e) = std::fs::write(&shortcuts_file, updated)
    {
        log::warn!("failed to write COSMIC shortcut: {e}");
    }
}

/// Everything that happens back in the user process after the root phase has
/// placed files. Computes the environment probes [`plan`] needs, gets back
/// an ordered [`Step`] list, and executes it through a thin per-step
/// `match` — that seam is what makes the whole decision tree ([`plan`]'s
/// unit tests) testable without ever touching the filesystem or a real
/// shell-out.
///
/// `applet_was_installed` must be captured *before* the root phase ran (the
/// script's `is_update` check) — it decides whether the panel needs
/// restarting to pick up a *changed* applet binary, not whether the applet
/// is present now.
///
/// # Errors
/// [`InstallError::PostInstallFailed`] only when the daemon restart/start
/// itself fails ([`Step::RestartOrStart`]) — every other step is best-effort
/// and only logs a warning.
pub async fn run(
    components: &Components,
    applet_was_installed: bool,
    interactive: bool,
    prefix: &Path,
) -> Result<(), InstallError> {
    let systemctl_available = on_path("systemctl");
    let cosmic_available = on_path("cosmic-panel");
    // Only worth the `pgrep` shell-out when it could actually matter —
    // `plan` re-checks the same `components.applet && applet_was_installed`
    // guard itself, so passing `false` when it doesn't apply is equivalent.
    let panel_running =
        components.applet && applet_was_installed && cmd_ok("pgrep", &["-f", "cosmic-panel"]).await;

    for step in plan(
        *components,
        applet_was_installed,
        interactive,
        systemctl_available,
        cosmic_available,
        panel_running,
    ) {
        match step {
            Step::DaemonReload => step_daemon_reload().await,
            Step::RemoveLegacyUnit => step_remove_legacy_unit(),
            Step::Enable => step_enable().await,
            Step::RestartOrStart => step_restart_or_start().await?,
            Step::RestartPanel => step_restart_panel().await,
            Step::NudgeLaunchers => step_nudge_launchers().await,
            Step::CleanupLegacy => cleanup_legacy(),
            Step::MigrateShortcut => migrate_shortcut(prefix),
            Step::PromptShortcut => prompt_shortcut(prefix),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const STT_CMD: &str = "/usr/local/bin/stt record --write";

    #[test]
    fn migrate_rewrites_legacy_path_only_when_present() {
        let c = r#"{ (key: "space"): Spawn("/home/u/.local/bin/stt record --write"), }"#;
        let out = migrate_shortcut_content(c, "/home/u", "/usr/local").unwrap();
        assert!(out.contains("/usr/local/bin/stt record --write"));
        assert!(!out.contains(".local/bin/stt "));
        assert!(migrate_shortcut_content("{}", "/home/u", "/usr/local").is_none());
    }

    #[test]
    fn add_skips_when_super_stt_or_super_space_exists() {
        assert!(
            shortcut_with_super_stt(r#"{ description: Some("Super STT") }"#, STT_CMD).is_none()
        );
        let taken = "{\n    (\n        modifiers: [\n            Super,\n        ],\n        key: \"space\",\n        description: Some(\"Other\"),\n    ): Spawn(\"x\"),\n}";
        assert!(shortcut_with_super_stt(taken, STT_CMD).is_none());
    }

    #[test]
    fn add_writes_full_file_when_empty_and_inserts_before_close_otherwise() {
        let fresh = shortcut_with_super_stt("", STT_CMD).unwrap();
        assert!(fresh.starts_with("{"));
        assert!(fresh.contains("Super STT"));
        assert!(fresh.trim_end().ends_with("}"));

        // The literal `{}`-only case (not just a truly empty file) also
        // gets the full-file template, per the doc comment's "Empty or
        // `{}`-only content" — not just whitespace-empty.
        let fresh_braces = shortcut_with_super_stt("{}", STT_CMD).unwrap();
        assert!(fresh_braces.contains("Super STT"));
        assert_eq!(
            fresh_braces.matches('}').count(),
            fresh_braces.matches('{').count()
        );

        let existing = "{\n    (\n        modifiers: [\n            Ctrl,\n        ],\n        key: \"t\",\n        description: Some(\"Terminal\"),\n    ): Spawn(\"term\"),\n}";
        let merged = shortcut_with_super_stt(existing, STT_CMD).unwrap();
        assert!(merged.contains("Terminal"));
        assert!(merged.contains("Super STT"));
        assert_eq!(merged.matches('}').count(), merged.matches('{').count());
        // Super STT entry comes after the existing one, before the final close.
        assert!(merged.rfind("Super STT").unwrap() > merged.find("Terminal").unwrap());
    }

    // --- plan(): the post-install decision tree, tested exhaustively so the
    // real shell-outs in `run`'s executor never need to be invoked to prove
    // the WHAT-to-do logic is right. ---

    fn all_three() -> Components {
        Components {
            daemon: true,
            app: true,
            applet: true,
        }
    }

    fn daemon_only() -> Components {
        Components {
            daemon: true,
            app: false,
            applet: false,
        }
    }

    #[test]
    fn plan_daemon_only_skips_launcher_nudge_and_applet_restart() {
        // Even with every environment probe favorable (systemctl+cosmic
        // available, applet "already installed", panel "running") — none of
        // that matters when the applet isn't part of THIS run's components.
        let steps = plan(daemon_only(), true, false, true, true, true);
        assert!(!steps.contains(&Step::NudgeLaunchers));
        assert!(!steps.contains(&Step::RestartPanel));
    }

    #[test]
    fn plan_applet_restart_requires_selected_installed_and_running_all_three() {
        let applet_only = Components {
            daemon: false,
            app: false,
            applet: true,
        };
        assert!(plan(applet_only, true, false, false, false, true).contains(&Step::RestartPanel));
        // Not previously installed (a fresh applet install, not an update):
        // no restart even if the panel happens to be running.
        assert!(!plan(applet_only, false, false, false, false, true).contains(&Step::RestartPanel));
        // Previously installed, but the panel isn't currently running:
        // nothing to restart.
        assert!(!plan(applet_only, true, false, false, false, false).contains(&Step::RestartPanel));
        // Applet not selected THIS run (e.g. a daemon-only update), even
        // though it was installed before and the panel is running.
        assert!(
            !plan(daemon_only(), true, false, false, false, true).contains(&Step::RestartPanel)
        );
    }

    #[test]
    fn plan_shortcut_steps_require_daemon_component_and_cosmic_available() {
        let daemon = daemon_only();

        // No cosmic-panel on PATH: no shortcut steps at all, interactive or not.
        let steps = plan(daemon, false, true, true, false, false);
        assert!(!steps.contains(&Step::MigrateShortcut));
        assert!(!steps.contains(&Step::PromptShortcut));

        // Cosmic available, non-interactive: migrate only (the prompt is
        // interactive-only, but migration isn't gated on it at all).
        let steps = plan(daemon, false, false, true, true, false);
        assert!(steps.contains(&Step::MigrateShortcut));
        assert!(!steps.contains(&Step::PromptShortcut));

        // Cosmic available AND interactive: both steps, prompt after migrate.
        let steps = plan(daemon, false, true, true, true, false);
        let migrate_at = steps.iter().position(|s| *s == Step::MigrateShortcut);
        let prompt_at = steps.iter().position(|s| *s == Step::PromptShortcut);
        assert!(migrate_at.is_some() && prompt_at.is_some());
        assert!(migrate_at < prompt_at);

        // App-only (no daemon component at all): no shortcut steps even with
        // cosmic available and an interactive session.
        let app_only = Components {
            daemon: false,
            app: true,
            applet: false,
        };
        let steps = plan(app_only, false, true, true, true, false);
        assert!(!steps.contains(&Step::MigrateShortcut));
        assert!(!steps.contains(&Step::PromptShortcut));
    }

    #[test]
    fn plan_no_systemctl_means_no_systemd_steps() {
        let steps = plan(all_three(), true, false, false, true, true);
        assert!(!steps.contains(&Step::DaemonReload));
        assert!(!steps.contains(&Step::RemoveLegacyUnit));
        assert!(!steps.contains(&Step::Enable));
        assert!(!steps.contains(&Step::RestartOrStart));
    }

    #[test]
    fn plan_no_daemon_component_means_no_systemd_steps_even_if_available() {
        let app_only = Components {
            daemon: false,
            app: true,
            applet: false,
        };
        let steps = plan(app_only, false, false, true, false, false);
        assert!(!steps.iter().any(|s| matches!(
            s,
            Step::DaemonReload | Step::RemoveLegacyUnit | Step::Enable | Step::RestartOrStart
        )));
    }

    #[test]
    fn plan_cleanup_legacy_always_runs_regardless_of_every_other_gate() {
        assert!(
            plan(Components::default(), false, false, false, false, false)
                .contains(&Step::CleanupLegacy)
        );
        assert!(plan(all_three(), true, true, true, true, true).contains(&Step::CleanupLegacy));
    }

    #[test]
    fn plan_app_only_nudges_launchers_with_no_systemd_or_shortcut_steps() {
        let app_only = Components {
            daemon: false,
            app: true,
            applet: false,
        };
        let steps = plan(app_only, false, false, true, true, false);
        assert!(steps.contains(&Step::NudgeLaunchers));
        assert!(!steps.iter().any(|s| matches!(
            s,
            Step::DaemonReload
                | Step::RestartOrStart
                | Step::MigrateShortcut
                | Step::PromptShortcut
        )));
    }

    #[test]
    fn plan_full_update_interactive_matches_scripts_step_order() {
        // Everything on: daemon+app+applet all selected (an "all" update),
        // the applet was already installed and its panel is currently
        // running, systemctl and cosmic-panel both available, interactive
        // session. This is `scripts/install-beta.sh`'s own order for the
        // "all" case: systemd install/enable/(re)start, applet panel
        // restart, launcher nudge, legacy cleanup, COSMIC shortcut
        // migrate-then-prompt.
        let steps = plan(all_three(), true, true, true, true, true);
        assert_eq!(
            steps,
            vec![
                Step::DaemonReload,
                Step::RemoveLegacyUnit,
                Step::Enable,
                Step::RestartOrStart,
                Step::RestartPanel,
                Step::NudgeLaunchers,
                Step::CleanupLegacy,
                Step::MigrateShortcut,
                Step::PromptShortcut,
            ]
        );
    }
}
