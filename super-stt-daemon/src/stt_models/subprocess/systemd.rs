// SPDX-License-Identifier: GPL-3.0-only
//! systemd `--user` transient-unit lifecycle for subprocess backends: spawning
//! the hardened sandbox unit, sweeping orphaned units left by a prior daemon
//! run, and the sandbox directives (shared with the enforcement test so they
//! stay in lock-step).

use std::path::Path;

use anyhow::{Context, Result, bail};
use log::{info, warn};

/// Stop any leftover `super-stt-backend-*` `--user` transient units. Called
/// at daemon startup as defense against a previous daemon run that exited
/// without running `Transcribe::shutdown()` (SIGKILL / panic /
/// `std::process::exit` skipping `Drop`). Each unit's name embeds the
/// spawning daemon's PID, so the current daemon can't reach old ones via
/// its normal unload path — sweeping by glob is the only deterministic
/// recovery. No-op when there are no matching units.
pub async fn cleanup_orphan_units() {
    // `systemctl --user list-units` is the safest enumerator: it includes
    // both active and failed transient units (so we can stop them all in
    // one shot) and is silent when nothing matches.
    let listing = match tokio::process::Command::new("systemctl")
        .args([
            "--user",
            "list-units",
            "--all",
            "--type=service",
            "--no-legend",
            "--plain",
            "super-stt-backend-*",
        ])
        .output()
        .await
    {
        Ok(o) if o.status.success() => o.stdout,
        Ok(o) => {
            warn!(
                "list-units for orphan sweep returned {}: {}",
                o.status,
                String::from_utf8_lossy(&o.stderr),
            );
            return;
        }
        Err(e) => {
            warn!("could not enumerate user units for orphan sweep: {e}");
            return;
        }
    };
    let listing = String::from_utf8_lossy(&listing);
    let units: Vec<String> = listing
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .filter(|u| u.starts_with("super-stt-backend-"))
        .collect();
    if units.is_empty() {
        return;
    }
    info!(
        "Sweeping {} orphaned backend unit(s) from a previous run",
        units.len()
    );
    for unit in units {
        match tokio::process::Command::new("systemctl")
            .args(["--user", "stop", &unit])
            .status()
            .await
        {
            Ok(s) if s.success() => info!("stopped orphan {unit}"),
            Ok(s) => warn!("systemctl --user stop {unit} exited with {s}"),
            Err(e) => warn!("failed to stop orphan {unit}: {e}"),
        }
    }
}

/// Spawn the backend binary in a hardened `systemd-run --user` transient unit.
pub(super) async fn spawn_systemd_unit(
    unit: &str,
    binary: &Path,
    backend_dir: &Path,
    socket_dir: &Path,
    socket: &Path,
) -> Result<()> {
    let mut cmd = tokio::process::Command::new("systemd-run");
    cmd.arg("--user")
        .arg(format!("--unit={unit}"))
        .arg("--quiet")
        // Garbage-collect the transient unit when it exits/fails.
        .arg("--collect");
    for param in hardening_params(backend_dir, socket_dir) {
        cmd.arg("-p").arg(param);
    }
    cmd.arg(format!(
        "--setenv=SUPER_STT_BACKEND_SOCKET={}",
        socket.display()
    ))
    .arg(format!(
        "--setenv=SUPER_STT_BACKEND_DIR={}",
        backend_dir.display()
    ))
    .arg("--setenv=RUST_LOG=info")
    .arg(binary);

    let output = cmd.output().await.context("failed to run systemd-run")?;
    if !output.status.success() {
        bail!(
            "systemd-run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    warn!("spawned sandboxed backend unit {unit}");
    Ok(())
}

/// The systemd sandbox directives applied to every spawned backend. Shared by
/// the spawner and the sandbox-enforcement tests so they stay in lock-step.
///
/// - `PrivateNetwork`: no network — the daemon already provisioned files.
/// - `ProtectSystem=strict` + `ProtectHome=read-only`: the filesystem is
///   read-only (the binary + model under `$HOME` stay visible read-only); only
///   the socket dir is writable.
/// - `PrivateTmp`, `NoNewPrivileges`, `SystemCallFilter=@system-service`.
/// - `DevicePolicy=closed` + `DeviceAllow`: only the GPU device nodes.
fn hardening_params(backend_dir: &Path, socket_dir: &Path) -> Vec<String> {
    vec![
        "PrivateNetwork=yes".to_string(),
        "ProtectSystem=strict".to_string(),
        "ProtectHome=read-only".to_string(),
        format!("ReadOnlyPaths={}", backend_dir.display()),
        format!("ReadWritePaths={}", socket_dir.display()),
        "PrivateTmp=yes".to_string(),
        "NoNewPrivileges=yes".to_string(),
        "DevicePolicy=closed".to_string(),
        "DeviceAllow=/dev/nvidia0 rw".to_string(),
        "DeviceAllow=/dev/nvidiactl rw".to_string(),
        "DeviceAllow=/dev/nvidia-uvm rw".to_string(),
        "DeviceAllow=/dev/nvidia-uvm-tools rw".to_string(),
        "DeviceAllow=/dev/dri/renderD128 rw".to_string(),
        "SystemCallFilter=@system-service".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::hardening_params;
    use std::path::PathBuf;

    /// Verify the systemd sandbox actually *enforces* its restrictions: a probe
    /// run under the same hardening as a real backend must be denied network,
    /// writes to `$HOME` and the system, host `/tmp` visibility, and
    /// non-allowed devices — while keeping `NoNewPrivileges` set and the socket
    /// dir writable.
    ///
    /// Requires a systemd user session; gated behind `SUPER_STT_TEST_SANDBOX=1`.
    #[tokio::test]
    async fn sandbox_is_enforced() {
        if std::env::var("SUPER_STT_TEST_SANDBOX").is_err() {
            return;
        }
        let home = std::env::var("HOME").expect("HOME");
        let base = PathBuf::from(&home).join(".cache/super-stt-sandbox-test");
        let backend_dir = base.join("backend");
        let socket_dir = base.join("sock");
        std::fs::create_dir_all(&backend_dir).unwrap();
        std::fs::create_dir_all(&socket_dir).unwrap();
        std::fs::write(backend_dir.join("backend.toml"), "probe = true\n").unwrap();

        // A host /tmp marker the private-tmp sandbox must NOT see.
        let marker = format!("/tmp/sbx-marker-{}", std::process::id());
        std::fs::write(&marker, "host").unwrap();

        let probe = format!(
            r#"
nonlo=$(awk -F: 'NR>2 {{ gsub(/ /,"",$1); if ($1 != "lo") print $1 }}' /proc/net/dev)
[ -z "$nonlo" ] && echo NET_ISOLATED || echo "NET_LEAK:$nonlo"
touch "$HOME/.sbx_$$" 2>/dev/null && {{ echo HOME_WRITABLE; rm -f "$HOME/.sbx_$$"; }} || echo HOME_RO
touch /etc/.sbx_$$ 2>/dev/null && echo ETC_WRITABLE || echo ETC_RO
grep -q "NoNewPrivs:[[:space:]]*1" /proc/self/status && echo NNP_SET || echo NNP_UNSET
[ -e "{marker}" ] && echo TMP_LEAK || echo TMP_PRIVATE
touch "{sock}/.sbx_$$" 2>/dev/null && {{ echo SOCK_RW; rm -f "{sock}/.sbx_$$"; }} || echo SOCK_RO
[ -r "{backend}/backend.toml" ] && echo BACKEND_READABLE || echo BACKEND_HIDDEN
if [ -e /dev/nvidia-modeset ]; then head -c1 /dev/nvidia-modeset >/dev/null 2>&1 && echo MODESET_OPEN || echo MODESET_BLOCKED; fi
"#,
            marker = marker,
            sock = socket_dir.display(),
            backend = backend_dir.display(),
        );

        let mut cmd = tokio::process::Command::new("systemd-run");
        cmd.arg("--user")
            .arg("--pipe")
            .arg("--quiet")
            .arg("--collect");
        for p in hardening_params(&backend_dir, &socket_dir) {
            cmd.arg("-p").arg(p);
        }
        cmd.arg("--").arg("sh").arg("-c").arg(&probe);

        let out = cmd.output().await.expect("run systemd-run");
        let _ = std::fs::remove_file(&marker);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        println!("--- probe stdout ---\n{stdout}\n--- stderr ---\n{stderr}");

        for expected in [
            "NET_ISOLATED",
            "HOME_RO",
            "ETC_RO",
            "NNP_SET",
            "TMP_PRIVATE",
            "SOCK_RW",
            "BACKEND_READABLE",
        ] {
            assert!(
                stdout.contains(expected),
                "sandbox property not enforced: {expected}\n{stdout}\n{stderr}"
            );
        }
        for violation in [
            "HOME_WRITABLE",
            "ETC_WRITABLE",
            "TMP_LEAK",
            "NET_LEAK",
            "MODESET_OPEN",
        ] {
            assert!(
                !stdout.contains(violation),
                "sandbox violation detected: {violation}\n{stdout}"
            );
        }
    }
}
