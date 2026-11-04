// SPDX-License-Identifier: GPL-3.0-only
//! Super Voice's own post-install steps: the COSMIC `Super+Space` shortcut to
//! `voice record --write`. They run after super-engine-installer's shared
//! steps, as [`after_install`], and are best-effort like them: a failure is
//! logged, and the install still succeeds.

use std::path::Path;

use super_engine_installer::{AfterInstall, Components};

/// One shortcut step, in the order [`plan`] returns them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Rewrite a shortcut that runs Super STT's `stt` to run `voice`.
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

/// Rewrite every shortcut that runs Super STT's `stt` — from wherever its
/// wrapper was installed, `~/.local/bin` or the prefix — to run
/// `<prefix>/bin/voice`, and rename its "Super STT" description. `stt` is
/// gone, so a shortcut left on it would do nothing. The arguments carry over:
/// `record` is `voice record` too, and `stt stop` is `voice record stop`.
/// Returns `None` when no shortcut runs `stt` (nothing to migrate).
#[must_use]
pub fn migrate_shortcut_content(content: &str, prefix: &str) -> Option<String> {
    const SPAWN: &str = "Spawn(\"";
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    let mut changed = false;
    while let Some(at) = rest.find(SPAWN) {
        let start = at + SPAWN.len();
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find('"').unwrap_or(rest.len());
        let command = &rest[..end];
        let program_end = command.find(' ').unwrap_or(command.len());
        let program = &command[..program_end];
        if program == "stt" || program.ends_with("/stt") {
            let args = &command[program_end..];
            let args = args
                .strip_prefix(" stop")
                .map_or_else(|| args.to_string(), |tail| format!(" record stop{tail}"));
            out.push_str(prefix);
            out.push_str("/bin/voice");
            out.push_str(&args);
            changed = true;
        } else {
            out.push_str(command);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    changed.then(|| {
        out.replace(
            "description: Some(\"Super STT\")",
            "description: Some(\"Super Voice\")",
        )
    })
}

/// Build the `Spawn(...)` shortcut entry block for `command`, RON-shaped
/// like the rest of the COSMIC shortcuts file.
fn shortcut_entry(command: &str) -> String {
    format!(
        "    (\n        modifiers: [\n            Super,\n        ],\n        key: \"space\",\n        description: Some(\"Super Voice\"),\n    ): Spawn(\"{command}\"),\n"
    )
}

/// Add a `Super+Space` → `command` shortcut entry to `content` (the
/// COSMIC custom-shortcuts file's current text), subject to two coarse
/// checks: skip (return `None`) if a "Super Voice" entry already exists, or if
/// `key: "space"` is already bound to a `Super`-modified shortcut
/// (approximated as both substrings appearing anywhere in `content`). Empty or
/// `{}`-only content gets the full-file template; otherwise the entry is
/// inserted before the final closing brace.
#[must_use]
pub fn shortcut_with_super_voice(content: &str, command: &str) -> Option<String> {
    if content.contains("Super Voice") {
        return None;
    }
    if content.contains("key: \"space\"") && content.contains("Super") {
        return None;
    }

    let entry = shortcut_entry(command);
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

/// Rewrite a shortcut in the COSMIC custom-shortcuts file that runs `stt`,
/// if there is one. See [`migrate_shortcut_content`].
fn migrate_shortcut(prefix: &Path) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let shortcuts_file =
        home.join(".config/cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom");
    if let Ok(content) = std::fs::read_to_string(&shortcuts_file)
        && let Some(migrated) = migrate_shortcut_content(&content, &prefix.to_string_lossy())
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
    let command = format!("{}/bin/voice record --write", prefix.display());
    let existing = std::fs::read_to_string(&shortcuts_file).unwrap_or_default();
    if let Some(updated) = shortcut_with_super_voice(&existing, &command)
        && let Err(e) = std::fs::write(&shortcuts_file, updated)
    {
        log::warn!("failed to write COSMIC shortcut: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VOICE_CMD: &str = "/usr/local/bin/voice record --write";

    /// Super STT's binding moves to `voice`, from either place its wrapper
    /// was installed, with its arguments kept.
    #[test]
    fn migrate_moves_an_stt_shortcut_to_voice() {
        for old in [
            "/home/u/.local/bin/stt record --write",
            "/usr/local/bin/stt record --write",
        ] {
            let c =
                format!(r#"{{ (key: "space", description: Some("Super STT")): Spawn("{old}"), }}"#);
            let out = migrate_shortcut_content(&c, "/usr/local").unwrap();
            assert!(
                out.contains(r#"Spawn("/usr/local/bin/voice record --write")"#),
                "{out}"
            );
            assert!(out.contains(r#"description: Some("Super Voice")"#), "{out}");
            assert!(!out.contains("/stt "), "{out}");
        }
    }

    /// `stt stop` was speech to text's; it is `voice record stop` now.
    #[test]
    fn migrate_moves_stt_stop_under_record() {
        let c = r#"{ (key: "s"): Spawn("/usr/local/bin/stt stop"), }"#;
        let out = migrate_shortcut_content(c, "/usr/local").unwrap();
        assert!(
            out.contains(r#"Spawn("/usr/local/bin/voice record stop")"#),
            "{out}"
        );
    }

    /// Nothing runs `stt`: nothing to migrate, and other shortcuts are left
    /// alone, including one whose program merely ends in `stt`-like text.
    #[test]
    fn migrate_leaves_other_shortcuts_alone() {
        assert!(migrate_shortcut_content("{}", "/usr/local").is_none());
        let c = r#"{ (key: "t"): Spawn("/usr/bin/gstt record"), (key: "u"): Spawn("term"), }"#;
        assert!(migrate_shortcut_content(c, "/usr/local").is_none());
    }

    #[test]
    fn add_skips_when_super_voice_or_super_space_exists() {
        assert!(
            shortcut_with_super_voice(r#"{ description: Some("Super Voice") }"#, VOICE_CMD)
                .is_none()
        );
        let taken = "{\n    (\n        modifiers: [\n            Super,\n        ],\n        key: \"space\",\n        description: Some(\"Other\"),\n    ): Spawn(\"x\"),\n}";
        assert!(shortcut_with_super_voice(taken, VOICE_CMD).is_none());
    }

    #[test]
    fn add_writes_full_file_when_empty_and_inserts_before_close_otherwise() {
        let fresh = shortcut_with_super_voice("", VOICE_CMD).unwrap();
        assert!(fresh.starts_with("{"));
        assert!(fresh.contains("Super Voice"));
        assert!(fresh.trim_end().ends_with("}"));

        // The literal `{}`-only case (not just a truly empty file) also
        // gets the full-file template, per the doc comment's "Empty or
        // `{}`-only content" — not just whitespace-empty.
        let fresh_braces = shortcut_with_super_voice("{}", VOICE_CMD).unwrap();
        assert!(fresh_braces.contains("Super Voice"));
        assert_eq!(
            fresh_braces.matches('}').count(),
            fresh_braces.matches('{').count()
        );

        let existing = "{\n    (\n        modifiers: [\n            Ctrl,\n        ],\n        key: \"t\",\n        description: Some(\"Terminal\"),\n    ): Spawn(\"term\"),\n}";
        let merged = shortcut_with_super_voice(existing, VOICE_CMD).unwrap();
        assert!(merged.contains("Terminal"));
        assert!(merged.contains("Super Voice"));
        assert_eq!(merged.matches('}').count(), merged.matches('{').count());
        // Super Voice's entry comes after the existing one, before the final close.
        assert!(merged.rfind("Super Voice").unwrap() > merged.find("Terminal").unwrap());
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
