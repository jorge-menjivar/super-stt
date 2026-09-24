// SPDX-License-Identifier: GPL-3.0-only
//! Super STT's own post-install steps: the COSMIC `Super+Space` shortcut to
//! `stt record --write`. They run after super-engine-installer's shared
//! steps, as [`after_install`], and are best-effort like them: a failure is
//! logged, and the install still succeeds.

use std::path::Path;

use super_engine_installer::{AfterInstall, Components};

/// One shortcut step, in the order [`plan`] returns them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Rewrite a legacy `~/.local/bin/stt` shortcut reference, if present.
    MigrateShortcut,
    /// Interactively offer to add the `Super+Space` shortcut.
    PromptShortcut,
}

/// Which shortcut steps to run: none unless the daemon (whose CLI the
/// shortcut runs) was installed on a COSMIC desktop, and the prompt only when
/// someone is at the terminal to answer it.
#[must_use]
pub fn plan(components: Components, cosmic_available: bool, interactive: bool) -> Vec<Step> {
    let mut steps = Vec::new();
    if components.daemon && cosmic_available {
        steps.push(Step::MigrateShortcut);
        if interactive {
            steps.push(Step::PromptShortcut);
        }
    }
    steps
}

/// The installer's [`Installer::after_install`](super_engine_installer::Installer::after_install).
pub fn after_install(done: &AfterInstall<'_>) {
    for step in plan(done.components, done.cosmic_available, done.interactive) {
        match step {
            Step::MigrateShortcut => migrate_shortcut(done.prefix),
            Step::PromptShortcut => prompt_shortcut(done.prefix),
        }
    }
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
/// COSMIC custom-shortcuts file's current text), subject to two coarse
/// checks: skip (return `None`) if a "Super STT" entry already exists, or if
/// `key: "space"` is already bound to a `Super`-modified shortcut
/// (approximated as both substrings appearing anywhere in `content`). Empty or
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

    fn daemon_only() -> Components {
        Components {
            daemon: true,
            app: false,
            applet: false,
        }
    }

    #[test]
    fn plan_shortcut_steps_require_daemon_component_and_cosmic_available() {
        let daemon = daemon_only();

        // No cosmic-panel on PATH: no shortcut steps at all, interactive or not.
        let steps = plan(daemon, false, true);
        assert!(!steps.contains(&Step::MigrateShortcut));
        assert!(!steps.contains(&Step::PromptShortcut));

        // Cosmic available, non-interactive: migrate only (the prompt is
        // interactive-only, but migration isn't gated on it at all).
        let steps = plan(daemon, true, false);
        assert!(steps.contains(&Step::MigrateShortcut));
        assert!(!steps.contains(&Step::PromptShortcut));

        // Cosmic available AND interactive: both steps, prompt after migrate.
        let steps = plan(daemon, true, true);
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
        let steps = plan(app_only, true, true);
        assert!(!steps.contains(&Step::MigrateShortcut));
        assert!(!steps.contains(&Step::PromptShortcut));
    }
}
