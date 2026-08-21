// SPDX-License-Identifier: GPL-3.0-only
//! Post-install steps run back in the unprivileged user process after the
//! root phase has placed files: systemd service (re)start, applet panel
//! restart, launcher-cache nudge, legacy `~/.local` cleanup, and the COSMIC
//! keyboard-shortcut migrate/add. A faithful port of `scripts/install-beta.sh`'s
//! equivalent steps, same order. Every step is best-effort (`log::warn!` on
//! failure) except the daemon restart, the one failure the app must hear
//! about.

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

/// `systemctl --user daemon-reload`; remove a legacy user-local unit that
/// would otherwise shadow the packaged one; `enable`; then `restart` (if
/// already active) or `start`. Only the final restart/start is a hard
/// error — reload/enable failures are logged and best-effort.
///
/// # Errors
/// [`InstallError::PostInstallFailed`] when the final `restart`/`start`
/// exits nonzero.
async fn run_systemd() -> Result<(), InstallError> {
    if !on_path("systemctl") {
        log::warn!("systemctl not found on PATH; skipping daemon service setup");
        return Ok(());
    }

    if !cmd_ok("systemctl", &["--user", "daemon-reload"]).await {
        log::warn!("systemctl --user daemon-reload failed");
    }

    // A unit left in ~/.config/systemd/user by an older install takes
    // precedence over the packaged one — remove it or systemd keeps
    // launching the (now deleted) legacy ~/.local/bin binary.
    if let Some(home) = dirs::home_dir() {
        let legacy_unit = home.join(".config/systemd/user/super-stt.service");
        if legacy_unit.exists() {
            let _ = std::fs::remove_file(&legacy_unit);
        }
    }

    if !cmd_ok("systemctl", &["--user", "enable", "super-stt"]).await {
        log::warn!("systemctl --user enable super-stt failed");
    }

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

/// Migrate (always) and, in interactive mode, offer to add the COSMIC
/// `Super+Space` shortcut. Gated by the caller on `components.daemon &&`
/// `cosmic-panel` on PATH.
fn cosmic_shortcut(interactive: bool, prefix: &Path) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let shortcuts_dir = home.join(".config/cosmic/com.system76.CosmicSettings.Shortcuts/v1");
    let shortcuts_file = shortcuts_dir.join("custom");

    if let Ok(content) = std::fs::read_to_string(&shortcuts_file)
        && let Some(migrated) =
            migrate_shortcut_content(&content, &home.to_string_lossy(), &prefix.to_string_lossy())
        && let Err(e) = std::fs::write(&shortcuts_file, migrated)
    {
        log::warn!("failed to migrate COSMIC shortcut: {e}");
    }

    if !interactive {
        return;
    }
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

/// Everything that happens back in the user process after the root phase
/// has placed files, in the script's order: systemd, applet panel restart,
/// launcher-cache nudge, legacy cleanup, COSMIC shortcut.
///
/// `applet_was_installed` must be captured *before* the root phase ran (the
/// script's `is_update` check) — it decides whether the panel needs
/// restarting to pick up a *changed* applet binary, not whether the applet
/// is present now.
///
/// # Errors
/// [`InstallError::PostInstallFailed`] only when the daemon restart/start
/// itself fails — every other step is best-effort and only logs a warning.
pub async fn run(
    components: &Components,
    applet_was_installed: bool,
    interactive: bool,
    prefix: &Path,
) -> Result<(), InstallError> {
    if components.daemon {
        run_systemd().await?;
    }

    if components.applet
        && applet_was_installed
        && cmd_ok("pgrep", &["-f", "cosmic-panel"]).await
        && !cmd_ok("pkill", &["-f", "cosmic-panel"]).await
    {
        log::warn!("failed to restart cosmic-panel to load the updated applet");
    }

    // Nudge COSMIC's launcher caches (app grid + search backend): both scan
    // desktop entries at session start and miss entries added to a
    // directory they weren't watching. They respawn on demand and rescan.
    let daemon_only = components.daemon && !components.app && !components.applet;
    if !daemon_only {
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

    cleanup_legacy();

    if components.daemon && on_path("cosmic-panel") {
        cosmic_shortcut(interactive, prefix);
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

        let existing = "{\n    (\n        modifiers: [\n            Ctrl,\n        ],\n        key: \"t\",\n        description: Some(\"Terminal\"),\n    ): Spawn(\"term\"),\n}";
        let merged = shortcut_with_super_stt(existing, STT_CMD).unwrap();
        assert!(merged.contains("Terminal"));
        assert!(merged.contains("Super STT"));
        assert_eq!(merged.matches('}').count(), merged.matches('{').count());
        // Super STT entry comes after the existing one, before the final close.
        assert!(merged.rfind("Super STT").unwrap() > merged.find("Terminal").unwrap());
    }
}
