// SPDX-License-Identifier: GPL-3.0-only
//! systemd `--user` transient-unit lifecycle for subprocess backends: spawning
//! the hardened sandbox unit, sweeping orphaned units left by a prior daemon
//! run, and the sandbox directives (shared with the enforcement test so they
//! stay in lock-step).

use std::path::Path;

use anyhow::{Context, Result, bail};
use log::{info, warn};

use crate::stt_models::backends::manifest::Device;

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
///
/// `devices` is the model's declared `supported_devices`; it decides whether
/// this unit is granted the GPU device nodes. See [`needs_gpu_access`].
pub(super) async fn spawn_systemd_unit(
    unit: &str,
    binary: &Path,
    backend_dir: &Path,
    socket_dir: &Path,
    socket: &Path,
    devices: &[Device],
) -> Result<()> {
    let mut cmd = tokio::process::Command::new("systemd-run");
    cmd.arg("--user")
        .arg(format!("--unit={unit}"))
        .arg("--quiet")
        // Garbage-collect the transient unit when it exits/fails.
        .arg("--collect");
    for param in hardening_params(backend_dir, socket_dir, needs_gpu_access(devices)) {
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

/// Whether a model that declares `devices` is granted the GPU device nodes.
///
/// Exposing `/dev/nvidia*` hands the backend privileged kernel attack surface,
/// so it is withheld from the models that provably cannot use it: those whose
/// `supported_devices` name no GPU.
///
/// The *runtime* device preference deliberately does not decide this. The
/// sandbox is fixed when the unit spawns, which happens before `load` — at
/// that point no preference for this model exists yet (the settings UI offers
/// the device picker only once a model is loaded), and the persisted default
/// would hide the GPU from a CUDA-only model. A model's declared capability is
/// known from `backend.toml` at spawn time and does not move afterwards, so a
/// GPU-capable model keeps the nodes whether or not the user currently prefers
/// the CPU — switching device must not depend on how the unit was spawned.
fn needs_gpu_access(devices: &[Device]) -> bool {
    devices
        .iter()
        .any(|d| matches!(d, Device::Cuda | Device::Metal))
}

/// The systemd sandbox directives applied to every spawned backend. Shared by
/// the spawner and the sandbox-enforcement tests so they stay in lock-step.
///
/// - `PrivateNetwork`: no network — the daemon already provisioned files.
/// - `ProtectSystem=strict` + `ProtectHome=read-only`: the filesystem is
///   read-only (the binary + model under `$HOME` stay visible read-only); only
///   the socket dir is writable.
/// - `PrivateTmp`, `NoNewPrivileges`, `SystemCallFilter=@system-service`.
/// - `PrivateDevices` (models declaring no GPU): a private `/dev` holding just
///   the pseudo-devices, so the GPU nodes are not there to open.
/// - `DevicePolicy=closed` + `DeviceAllow`: the intended device allowlist.
///
/// The device *cgroup* controller (`DevicePolicy`/`DeviceAllow`) is only
/// enforced for system units — a per-user manager records the properties and
/// runs the unit without them, because installing the BPF device program is
/// privileged. They are declared anyway so the policy is right if these units
/// ever move to the system manager, but the CPU-only restriction cannot lean
/// on them. `PrivateDevices` carries it instead: it is a mount namespace, so
/// it applies to user units exactly as `PrivateTmp` does.
fn hardening_params(backend_dir: &Path, socket_dir: &Path, gpu_access: bool) -> Vec<String> {
    let mut params = vec![
        "PrivateNetwork=yes".to_string(),
        "ProtectSystem=strict".to_string(),
        "ProtectHome=read-only".to_string(),
        format!("ReadOnlyPaths={}", backend_dir.display()),
        format!("ReadWritePaths={}", socket_dir.display()),
        "PrivateTmp=yes".to_string(),
        "NoNewPrivileges=yes".to_string(),
        "DevicePolicy=closed".to_string(),
    ];
    if gpu_access {
        params.extend(
            [
                "DeviceAllow=/dev/nvidia0 rw",
                "DeviceAllow=/dev/nvidiactl rw",
                "DeviceAllow=/dev/nvidia-uvm rw",
                "DeviceAllow=/dev/nvidia-uvm-tools rw",
                "DeviceAllow=/dev/dri/renderD128 rw",
            ]
            .map(String::from),
        );
    } else {
        params.push("PrivateDevices=yes".to_string());
    }
    params.push("SystemCallFilter=@system-service".to_string());
    params
}

#[cfg(test)]
mod tests {
    use super::{Device, hardening_params, needs_gpu_access};
    use std::path::{Path, PathBuf};

    /// A model that declares no GPU device must not be handed the GPU nodes.
    /// The GPU driver is privileged kernel attack surface, so the sandbox
    /// opens it only for the backends that can actually compute on it.
    ///
    /// `PrivateDevices` is what makes this real: the daemon spawns `--user`
    /// units, and a per-user manager does not enforce the device cgroup
    /// controller, so dropping the `DeviceAllow` lines alone would restrict
    /// nothing. `sandbox_is_enforced` proves the denial end-to-end.
    #[test]
    fn a_cpu_only_model_gets_no_gpu_device_nodes() {
        let params = hardening_params(Path::new("/backend"), Path::new("/sock"), false);
        assert!(
            params.iter().any(|p| p == "PrivateDevices=yes"),
            "cpu-only unit has no enforceable device restriction: {params:?}"
        );
        assert!(
            !params.iter().any(|p| p.starts_with("DeviceAllow=")),
            "cpu-only unit was granted device nodes: {params:?}"
        );
    }

    /// The GPU path still gets every node CUDA needs, and must NOT get the
    /// private `/dev` — that would hide the GPU from the one backend that
    /// needs it, failing at model-load time as an opaque driver error rather
    /// than as a permission problem.
    #[test]
    fn a_gpu_backend_gets_the_cuda_device_nodes() {
        let params = hardening_params(Path::new("/backend"), Path::new("/sock"), true);
        for node in [
            "DeviceAllow=/dev/nvidia0 rw",
            "DeviceAllow=/dev/nvidiactl rw",
            "DeviceAllow=/dev/nvidia-uvm rw",
            "DeviceAllow=/dev/nvidia-uvm-tools rw",
            "DeviceAllow=/dev/dri/renderD128 rw",
        ] {
            assert!(params.iter().any(|p| p == node), "missing {node}");
        }
        assert!(
            !params.iter().any(|p| p == "PrivateDevices=yes"),
            "gpu backend was given a private /dev, hiding the GPU: {params:?}"
        );
    }

    /// Pin the `supported_devices` → GPU-access mapping. A model keeps the
    /// nodes if *any* declared device is a GPU, because the user may switch to
    /// it after load; only a model that names no GPU at all — CPU-only, or the
    /// `none` sentinel of a remote model — is spawned without them.
    #[test]
    fn only_a_model_declaring_a_gpu_gets_gpu_access() {
        assert!(!needs_gpu_access(&[Device::Cpu]));
        assert!(!needs_gpu_access(&[Device::None]));
        assert!(!needs_gpu_access(&[]));
        assert!(needs_gpu_access(&[Device::Cuda]));
        assert!(needs_gpu_access(&[Device::Metal]));
        assert!(needs_gpu_access(&[Device::Cpu, Device::Cuda]));
    }

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
[ -e /dev/nvidia0 ] && echo GPU_NODE_PRESENT || echo GPU_NODE_ABSENT
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
        for p in hardening_params(&backend_dir, &socket_dir, true) {
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
        for violation in ["HOME_WRITABLE", "ETC_WRITABLE", "TMP_LEAK", "NET_LEAK"] {
            assert!(
                !stdout.contains(violation),
                "sandbox violation detected: {violation}\n{stdout}"
            );
        }

        // The GPU half of the check needs a host that actually has the node —
        // otherwise "absent under the CPU params" is true for the wrong reason
        // and proves nothing.
        if !Path::new("/dev/nvidia0").exists() {
            return;
        }
        assert!(
            stdout.contains("GPU_NODE_PRESENT"),
            "a GPU backend cannot see the GPU it was granted\n{stdout}\n{stderr}"
        );

        // Same probe under the params a model declaring no GPU would get.
        // Asserting on the parameter list alone would only prove we omit a
        // string; this proves systemd enforces the omission — and it is the
        // reason that case carries `PrivateDevices` rather than just dropping
        // the `DeviceAllow` lines, which a per-user manager ignores.
        let mut cmd = tokio::process::Command::new("systemd-run");
        cmd.arg("--user")
            .arg("--pipe")
            .arg("--quiet")
            .arg("--collect");
        for p in hardening_params(&backend_dir, &socket_dir, false) {
            cmd.arg("-p").arg(p);
        }
        cmd.arg("--").arg("sh").arg("-c").arg(&probe);

        let out = cmd.output().await.expect("run systemd-run");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("GPU_NODE_ABSENT"),
            "a model declaring no GPU was left the GPU node\n{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
