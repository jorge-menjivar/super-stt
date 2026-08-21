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
    let out = tokio::process::Command::new(escalator)
        .arg(&me)
        .arg("--root-phase")
        .arg(manifest_path)
        .stdin(std::process::Stdio::inherit())
        .output()
        .await
        .map_err(|e| InstallError::EscalationUnavailable(e.to_string()))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    match (escalator, out.status.code()) {
        // pkexec: 126 = dialog dismissed, 127 = not authorized.
        ("pkexec", Some(126 | 127)) => Err(InstallError::EscalationDenied),
        ("sudo", Some(1)) if stderr.contains("incorrect password") || stderr.contains("Sorry") => {
            Err(InstallError::EscalationDenied)
        }
        _ => Err(InstallError::InstallFailed(format!(
            "root phase exited {:?}: {}",
            out.status.code(),
            stderr.trim()
        ))),
    }
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
}
