// SPDX-License-Identifier: GPL-3.0-only
//! Privilege escalation: pick `sudo` (TTY) or `pkexec` (no TTY / GUI) and
//! re-exec this same binary under it for the `--root-phase` step.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::errors::InstallError;

/// Which escalator was chosen to re-exec the root phase under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Sudo,
    Pkexec,
}

/// Scan `path_env` (a `:`-separated `$PATH`-shaped string) for an executable
/// file named `bin`, returning the first match. Pure — takes `path_env`
/// explicitly rather than reading `$PATH` itself, so it's testable without
/// mutating process environment.
#[must_use]
pub fn which(bin: &str, path_env: &str) -> Option<PathBuf> {
    for dir in path_env.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(bin);
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if meta.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}

/// Choose the escalation method: `sudo` on a TTY (it can prompt for a
/// password interactively), else `pkexec` (its polkit agent shows its own
/// GUI dialog and doesn't need a controlling terminal).
///
/// # Errors
/// [`InstallError::EscalationUnavailable`] naming what's missing when
/// neither a usable `sudo` (with a TTY) nor `pkexec` is available.
pub fn pick_method(
    stderr_is_tty: bool,
    has_sudo: bool,
    has_pkexec: bool,
) -> Result<Method, InstallError> {
    if stderr_is_tty && has_sudo {
        return Ok(Method::Sudo);
    }
    if has_pkexec {
        return Ok(Method::Pkexec);
    }
    let reason = if has_sudo {
        "no controlling terminal for sudo, and pkexec is not installed"
    } else {
        "neither sudo nor pkexec is available"
    };
    Err(InstallError::EscalationUnavailable(reason.to_string()))
}

/// Classify a non-zero exit from the escalated `<escalator> <exe> --root-phase
/// <manifest>` command into the wire-contract error the app branches on
/// (extracted from `run_root_phase` per F2 so the whole denial matrix is
/// testable without ever invoking a real `sudo`/`pkexec`).
///
/// `stderr` is whatever text was actually captured for the failing process —
/// pass `""` when it was inherited instead (the `Method::Sudo` case; see
/// `run_root_phase`'s F3 doc comment). Callers must have already forced
/// `LANG=C`/`LC_ALL=C` on the escalated command (F1) so any captured `stderr`
/// text this matches against is guaranteed English, not gettext-localized.
///
/// # Mapping (C1: exit code `3` is `crate::root_phase::run`'s OWN failure
/// code, distinct from either escalator's denial codes — see that
/// function's doc comment for why. That's what makes the rest of this
/// mapping sound: an escalator's denial code and "the root phase ran and
/// failed" can never collide on the same exit code.)
/// - `pkexec` exit 126 (dialog dismissed) or 127 (not authorized) →
///   [`InstallError::EscalationDenied`].
/// - `sudo` exit 1, when `stderr` is empty or names a denial
///   (`"incorrect password"`/`"Sorry"`) → [`InstallError::EscalationDenied`].
///   Exit 1 is sudo's own refusal-or-cannot-run status — a bad/missing
///   password, but also a sudoers/config problem or a failure to exec the
///   command at all — rather than the invoked command's exit status, so we
///   treat it as a denial; the root phase's own failures are distinguishable
///   because they exit `3`, never `1`. The empty-`stderr` case is the
///   inherited-stdio case (F3): `stderr` is always `""` here for a real
///   invocation, since `Method::Sudo` inherits stderr rather than capturing
///   it.
/// - either escalator, exit `3` → [`InstallError::InstallFailed`]: the root
///   phase itself ran and failed. Carries the captured stderr when there is
///   any (pkexec always captures it); when `stderr` is empty instead, the
///   message is escalator-aware: for `sudo` (which inherits stderr rather
///   than capturing it — F3) it points at the terminal output above; for
///   `pkexec` (no terminal to point at) it says only that no reason was
///   reported — practically unreachable, since the root phase always prints
///   before exiting `3` and pkexec always captures that output.
/// - anything else → [`InstallError::InstallFailed`] naming the exit code and
///   trimmed stderr.
#[must_use]
pub fn classify_failure(escalator: &str, code: Option<i32>, stderr: &str) -> InstallError {
    match (escalator, code) {
        ("pkexec", Some(126 | 127)) => InstallError::EscalationDenied,
        ("sudo", Some(1))
            if stderr.is_empty()
                || stderr.contains("incorrect password")
                || stderr.contains("Sorry") =>
        {
            InstallError::EscalationDenied
        }
        // C1: `root_phase::run`'s own failure code, for either escalator —
        // never a denial, regardless of which escalator propagated it.
        (_, Some(3)) if stderr.is_empty() => {
            let msg = if escalator == "sudo" {
                "the root phase failed; see the terminal output above for the reason"
            } else {
                "the root phase failed without reporting a reason"
            };
            InstallError::InstallFailed(msg.to_string())
        }
        (_, Some(3)) => {
            InstallError::InstallFailed(format!("root phase failed: {}", stderr.trim()))
        }
        _ => InstallError::InstallFailed(format!("root phase exited {code:?}: {}", stderr.trim())),
    }
}

/// Re-exec this same running binary (`std::env::current_exe()`) under
/// `method`, invoking `<exe> --root-phase <manifest_path>`. Blocks until the
/// escalated process exits.
///
/// # Errors
/// [`InstallError::EscalationDenied`] when the user dismissed the pkexec
/// dialog (exit 126), was refused authorization (exit 127), or typed a wrong
/// sudo password; [`InstallError::EscalationUnavailable`] when the escalator
/// itself could not be spawned (e.g. no polkit agent running); otherwise
/// [`InstallError::InstallFailed`] naming the escalated process's exit code
/// and stderr.
pub async fn run_root_phase(method: Method, manifest_path: &Path) -> Result<(), InstallError> {
    let me = std::env::current_exe()
        .map_err(|e| InstallError::InstallFailed(format!("current_exe: {e}")))?;
    if matches!(method, Method::Sudo) {
        // Prime the sudo timestamp so the actual run doesn't re-prompt oddly.
        let ok = tokio::process::Command::new("sudo")
            .arg("-v")
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .env_remove("LANGUAGE")
            .status()
            .await
            .map_err(|e| InstallError::EscalationUnavailable(e.to_string()))?;
        if !ok.success() {
            return Err(InstallError::EscalationDenied);
        }
    }
    let escalator = match method {
        Method::Sudo => "sudo",
        Method::Pkexec => "pkexec",
    };
    let mut cmd = tokio::process::Command::new(escalator);
    cmd.arg(&me)
        .arg("--root-phase")
        .arg(manifest_path)
        .stdin(std::process::Stdio::inherit())
        // F1: sudo/pkexec localize their diagnostics via gettext — force
        // English so `classify_failure`'s denial-phrase match below isn't
        // locale-dependent (a French/German/Spanish `LANG` would otherwise
        // misreport a wrong-password rejection as `InstallFailed`).
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env_remove("LANGUAGE");

    let (status, stderr) = match method {
        Method::Sudo => {
            // F3: `pick_method` only ever returns `Sudo` when stderr is a
            // TTY, so inherit it here instead of capturing via `.output()`:
            // if the `sudo -v` primer's timestamp expires in the window
            // between it and this call, sudo re-prompts, and an inherited
            // prompt is visible — a captured one is an invisible prompt into
            // a pipe nobody answers, hanging the process forever.
            cmd.stderr(std::process::Stdio::inherit());
            let status = cmd
                .status()
                .await
                .map_err(|e| InstallError::EscalationUnavailable(e.to_string()))?;
            (status, String::new())
        }
        Method::Pkexec => {
            // No TTY here by construction (`pick_method` only picks Pkexec
            // when sudo can't prompt), so there's no re-prompt-into-a-pipe
            // risk — capture stderr as before for classification.
            let out = cmd
                .output()
                .await
                .map_err(|e| InstallError::EscalationUnavailable(e.to_string()))?;
            (
                out.status,
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        }
    };
    if status.success() {
        return Ok(());
    }
    Err(classify_failure(escalator, status.code(), &stderr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A fresh, empty per-test temp directory (per-pid, plus a per-call
    /// counter so parallel tests in this binary never collide).
    fn test_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("sstt-install-escalate-{}-{n}", std::process::id()));
        // F6: clear a pre-existing directory first — the pid+counter name
        // is only unique within one process run, so PID reuse across
        // separate test-binary invocations could otherwise leak files from
        // a previous run into this one.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn which_scans_path_entries() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("fakebin");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path_env = format!("/nonexistent:{}", dir.display());
        assert_eq!(which("fakebin", &path_env), Some(exe));
        assert_eq!(which("missing", &path_env), None);
    }

    #[test]
    fn method_choice_matrix() {
        assert!(matches!(pick_method(true, true, true), Ok(Method::Sudo)));
        assert!(matches!(pick_method(true, false, true), Ok(Method::Pkexec)));
        assert!(matches!(pick_method(false, true, true), Ok(Method::Pkexec))); // no TTY -> sudo can't prompt
        assert!(matches!(
            pick_method(false, true, false),
            Err(InstallError::EscalationUnavailable(_))
        ));
    }

    #[test]
    fn which_ignores_a_non_executable_file() {
        let dir = test_dir();
        let f = dir.join("not-executable");
        std::fs::write(&f, b"nope").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        let path_env = dir.display().to_string();
        assert_eq!(which("not-executable", &path_env), None);
    }

    // --- classify_failure (F2): pure, so the whole denial matrix is
    // testable without ever invoking a real sudo/pkexec. ---

    #[test]
    fn classify_pkexec_dialog_dismissed_or_unauthorized_as_denied() {
        assert!(matches!(
            classify_failure("pkexec", Some(126), ""),
            InstallError::EscalationDenied
        ));
        assert!(matches!(
            classify_failure("pkexec", Some(127), ""),
            InstallError::EscalationDenied
        ));
    }

    #[test]
    fn classify_sudo_denial_with_english_stderr() {
        // F1: LANG=C/LC_ALL=C on the escalated command guarantees sudo's
        // diagnostics are English before we ever get here to match them.
        assert!(matches!(
            classify_failure("sudo", Some(1), "Sorry, try again.\n"),
            InstallError::EscalationDenied
        ));
        assert!(matches!(
            classify_failure("sudo", Some(1), "sudo: 1 incorrect password attempt\n"),
            InstallError::EscalationDenied
        ));
    }

    #[test]
    fn classify_sudo_denial_with_inherited_empty_stderr() {
        // F3: for `Method::Sudo` stderr is inherited (visible to the user),
        // not captured — `run_root_phase` passes `""` in that case. C1: a
        // sudo exit code of 1 is unambiguously a denial regardless of
        // stderr content — `root_phase::run` never exits 1 (it exits `3` on
        // failure), so 1 can only mean sudo itself refused to run the
        // command at all.
        assert!(matches!(
            classify_failure("sudo", Some(1), ""),
            InstallError::EscalationDenied
        ));
    }

    #[test]
    fn classify_pkexec_other_code_is_install_failed_with_details() {
        let e = classify_failure("pkexec", Some(1), "some polkit error\n");
        match e {
            InstallError::InstallFailed(msg) => {
                assert!(msg.contains('1'), "{msg}");
                assert!(msg.contains("some polkit error"), "{msg}");
            }
            other => panic!("expected InstallFailed, got {other:?}"),
        }
    }

    #[test]
    fn classify_sudo_other_code_is_install_failed_with_details() {
        let e = classify_failure("sudo", Some(2), "unexpected failure\n");
        match e {
            InstallError::InstallFailed(msg) => {
                assert!(msg.contains('2'), "{msg}");
                assert!(msg.contains("unexpected failure"), "{msg}");
                assert!(!msg.ends_with('\n'), "stderr must be trimmed: {msg:?}");
            }
            other => panic!("expected InstallFailed, got {other:?}"),
        }
    }

    // --- C1: `root_phase::run`'s own failure code (3), distinct from
    // sudo's/pkexec's own escalator-denial codes, so a root-phase failure
    // (containment rejection, disk full, missing staged source, ...) is
    // never misclassified as the user having declined authorization. ---

    #[test]
    fn classify_root_phase_failure_exit_code_is_install_failed_not_denied() {
        // Exit 3 is `root_phase::run`'s OWN failure code, propagated
        // verbatim by both escalators — it must always mean "the root phase
        // ran and failed", never "authorization was denied".
        for escalator in ["sudo", "pkexec"] {
            let e = classify_failure(escalator, Some(3), "");
            assert!(
                matches!(e, InstallError::InstallFailed(_)),
                "{escalator}: expected InstallFailed, got {e:?}"
            );
        }
    }

    #[test]
    fn classify_root_phase_failure_carries_captured_stderr() {
        // pkexec's stderr is captured (not inherited) — the real error the
        // root phase printed must survive into the reported message.
        let e = classify_failure("pkexec", Some(3), "error: staging missing foo\n");
        match e {
            InstallError::InstallFailed(msg) => {
                assert!(msg.contains("staging missing foo"), "{msg}");
            }
            other => panic!("expected InstallFailed, got {other:?}"),
        }
    }

    #[test]
    fn classify_root_phase_failure_with_inherited_stderr_points_at_the_terminal() {
        // sudo's stderr is always inherited (never captured — see
        // `run_root_phase`'s F3 doc comment), so `classify_failure` sees an
        // empty string here for a real invocation. The message must still
        // tell the user something useful: that the real error was already
        // printed to their terminal, not just "root phase exited Some(3): ".
        let e = classify_failure("sudo", Some(3), "");
        match e {
            InstallError::InstallFailed(msg) => {
                assert!(
                    msg.to_lowercase().contains("terminal"),
                    "expected a message pointing at the inherited terminal output: {msg}"
                );
            }
            other => panic!("expected InstallFailed, got {other:?}"),
        }
    }
}
