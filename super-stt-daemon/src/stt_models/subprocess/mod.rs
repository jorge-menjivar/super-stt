// SPDX-License-Identifier: GPL-3.0-only
//! Host for STT backends shipped as sandboxed native subprocesses
//! (experimental — gated behind the `subprocess-backends` feature).
//!
//! [`SubprocessBackend`] provisions a backend's model files (downloading from
//! `HuggingFace` into the per-backend directory), spawns the backend binary in a
//! hardened `systemd-run --user` transient unit, drives the `/v1` contract
//! over a pathname Unix socket, and presents the result through the daemon's
//! [`Transcribe`] trait. The backend itself is fully self-contained and shares
//! no code with the daemon.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use log::{info, warn};
use tokio::net::UnixStream;

use super_stt_shared::utils::audio::{ResampleQuality, resample};

use crate::stt_models::backends::manifest::Manifest;
use crate::stt_models::transcribe::{ModelInfo, ModelInfoData, ModelState, Transcribe};

const SAMPLE_RATE: u32 = 16000;

/// A running, sandboxed subprocess backend usable as a [`Transcribe`] model.
pub struct SubprocessBackend {
    socket: PathBuf,
    unit: String,
    model_id: String,
    info: ModelInfoData,
    /// Device label reported by the backend's `/v1/status` (e.g. `"cuda"`).
    device: String,
}

impl SubprocessBackend {
    /// Provision the selected model, spawn the sandboxed backend, and load it.
    ///
    /// `backend_dir` holds `backend.toml` and the `entrypoint` binary; model
    /// files are downloaded into `<backend_dir>/<dest>`. `device_pref` is
    /// `"cpu"`, `"cuda"`, or empty (auto).
    ///
    /// # Errors
    /// Returns an error if provisioning, spawning, or loading fails.
    pub async fn spawn(
        backend_dir: &Path,
        model_name: &str,
        device_pref: &str,
        tracker: Option<&Arc<crate::download_progress::DownloadProgressTracker>>,
    ) -> Result<Self> {
        let manifest = Manifest::load(backend_dir)?;

        let model = manifest
            .models
            .iter()
            .find(|m| m.name == model_name)
            .ok_or_else(|| anyhow!("model {model_name} not declared in backend.toml"))?;

        // Provision ONLY the selected model's files (lazy per model). The
        // tracker (when present) reports per-file and per-byte progress through
        // `DownloadStateManager` so the settings app's progress bar updates in
        // real time. Each file carries its own URL and destination; `parse`
        // already validated every `destination` as a safe relative path, so the
        // join below cannot escape the backend dir.
        let items: Vec<_> = model
            .files
            .iter()
            .map(|spec| crate::stt_models::download::DownloadItem {
                url: spec.url.clone(),
                destination: backend_dir.join(&spec.destination),
                sha256: spec.sha256.clone(),
            })
            .collect();
        info!(
            "provisioning {model_name}: {} files into {}",
            items.len(),
            backend_dir.display()
        );
        crate::stt_models::download::download_files(&items, tracker, 0)
            .await
            .with_context(|| format!("provisioning {model_name}"))?;

        // All files are on disk. Spawning the sandboxed unit and loading
        // weights onto the device is the slow tail (tens of seconds for a
        // multi-GB model on GPU) but isn't byte-tracked — flip the tracker
        // to "loading_model" so the settings app swaps the full download
        // bar for a "Loading model into memory…" indicator instead of
        // freezing on a full bar.
        if let Some(t) = tracker {
            t.mark_loading();
            t.broadcast_progress();
        }

        // Socket under the runtime dir (pathname socket — survives PrivateNetwork).
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let socket_dir = PathBuf::from(&runtime).join("stt/backends");
        std::fs::create_dir_all(&socket_dir)?;
        let socket = socket_dir.join(format!("{}.sock", sanitize(model_name)));
        let _ = std::fs::remove_file(&socket);

        let binary = backend_dir.join(&manifest.backend.entrypoint);
        anyhow::ensure!(
            binary.exists(),
            "backend binary not found: {}",
            binary.display()
        );

        let unit = format!(
            "super-stt-backend-{}-{}",
            sanitize(model_name),
            std::process::id()
        );

        spawn_systemd_unit(&unit, &binary, backend_dir, &socket_dir, &socket).await?;

        let interval = model
            .processing_interval_ms
            .map_or_else(|| Duration::from_secs(2), Duration::from_millis);
        let info = ModelInfoData::new(
            model_name,
            model.provider.clone(),
            manifest.backend.source.clone(),
            model.multilingual,
            model.is_online(),
            interval,
        );

        let mut backend = Self {
            socket,
            unit,
            model_id: model_name.to_string(),
            info,
            device: "unknown".to_string(),
        };

        let provider_str = model.provider.to_string();
        backend.wait_for_ping(Duration::from_secs(30)).await?;
        backend.load(model_name, &provider_str, device_pref).await?;
        Ok(backend)
    }

    /// Poll `/v1/ping` until the backend is serving or the deadline passes.
    async fn wait_for_ping(&self, timeout: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Ok((200, _)) = self.request("GET", "/v1/ping", &[], Vec::new()).await {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                bail!(
                    "backend did not start within {timeout:?}.\n{}",
                    self.unit_logs()
                );
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// `POST /v1/load` then poll `/v1/status` until `ready` (or `error`),
    /// capturing the device label the backend reports.
    async fn load(&mut self, name: &str, provider: &str, device_pref: &str) -> Result<()> {
        let mut load = serde_json::json!({ "name": name, "provider": provider });
        if !device_pref.is_empty() {
            load["device"] = serde_json::json!(device_pref);
        }
        let body = serde_json::to_vec(&load)?;
        let (status, resp) = self
            .request("POST", "/v1/load", &json_headers(), body)
            .await?;
        anyhow::ensure!(
            status == 202 || status == 200,
            "/v1/load returned {status}: {}",
            String::from_utf8_lossy(&resp)
        );

        // Loading the model onto the GPU can take a while.
        let deadline = std::time::Instant::now() + Duration::from_mins(10);
        loop {
            let (_, resp) = self.request("GET", "/v1/status", &[], Vec::new()).await?;
            let json: serde_json::Value = serde_json::from_slice(&resp)?;
            match json.get("state").and_then(|v| v.as_str()) {
                Some("ready") => {
                    let device = json.get("device").and_then(|v| v.as_str()).unwrap_or("?");
                    info!("backend ready (device={device})");
                    self.device = device.to_string();
                    return Ok(());
                }
                Some("error") => bail!(
                    "backend load failed: {}",
                    json.get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                ),
                _ => {}
            }
            if std::time::Instant::now() >= deadline {
                bail!("backend load timed out");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// One HTTP request over the backend's Unix socket.
    async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Vec<u8>,
    ) -> Result<(u16, Vec<u8>)> {
        let stream = UnixStream::connect(&self.socket)
            .await
            .with_context(|| format!("connect {}", self.socket.display()))?;
        let io = TokioIo::new(stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let mut builder = hyper::Request::builder()
            .method(method)
            .uri(path)
            .header("host", "backend.local");
        for (k, v) in headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        let req = builder.body(Full::new(Bytes::from(body)))?;

        let resp = sender.send_request(req).await?;
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await?.to_bytes().to_vec();
        Ok((status, bytes))
    }

    /// Capture recent unit logs for diagnostics.
    fn unit_logs(&self) -> String {
        std::process::Command::new("journalctl")
            .args(["--user", "-u", &self.unit, "--no-pager", "-n", "30"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }
}

impl Drop for SubprocessBackend {
    fn drop(&mut self) {
        // Best-effort: stop the transient unit (SIGTERM) and remove the socket.
        //
        // `Drop` is synchronous, so we call `std::process::Command` directly;
        // it blocks the runtime worker thread while `systemctl --user stop`
        // waits for the unit to exit (usually under a second). Surfaces the
        // result so a failure doesn't silently leave the subprocess running
        // — the previous `let _ = …` swallowed every error, which made the
        // "backend not stopping" failure mode invisible.
        match std::process::Command::new("systemctl")
            .args(["--user", "stop", &self.unit])
            .status()
        {
            Ok(status) if status.success() => {
                info!("stopped backend unit {}", self.unit);
            }
            Ok(status) => {
                warn!(
                    "systemctl --user stop {} exited with {}; subprocess may still be running",
                    self.unit, status,
                );
            }
            Err(e) => {
                warn!("failed to invoke systemctl to stop {}: {e}", self.unit);
            }
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

impl ModelInfo for SubprocessBackend {
    fn info(&self) -> &ModelInfoData {
        &self.info
    }
}

impl ModelState for SubprocessBackend {
    /// Device label the backend reported at load time (e.g. `"cuda"`).
    fn device(&self) -> String {
        self.device.clone()
    }
}

#[async_trait]
impl Transcribe for SubprocessBackend {
    /// Stop the `systemd-run --user` transient unit asynchronously and
    /// remove the socket file. Called by the daemon before the
    /// [`LoadedModel`](crate::daemon::types::LoadedModel) is dropped — gives
    /// us a real `.await` instead of blocking the runtime in `Drop`. After
    /// this returns, the synchronous `Drop` impl is effectively a no-op
    /// (the unit is already stopped) and stays for crash paths and tests.
    async fn shutdown(&mut self) -> Result<()> {
        let status = tokio::process::Command::new("systemctl")
            .args(["--user", "stop", &self.unit])
            .status()
            .await
            .with_context(|| format!("invoke systemctl to stop {}", self.unit))?;
        if status.success() {
            info!("stopped backend unit {}", self.unit);
        } else {
            warn!(
                "systemctl --user stop {} exited with {status}; subprocess may still be running",
                self.unit,
            );
        }
        let _ = std::fs::remove_file(&self.socket);
        Ok(())
    }

    async fn transcribe_audio(&mut self, audio: &[f32], sample_rate: u32) -> Result<String> {
        // The daemon owns resampling; backends receive 16 kHz.
        let audio16 = resample(audio, sample_rate, SAMPLE_RATE, ResampleQuality::Fast)?;
        let body = serde_json::to_vec(&serde_json::json!({
            "audio_data": audio16,
            "sample_rate": SAMPLE_RATE,
        }))?;
        let mut headers = json_headers();
        headers.push(("x-stt-model".to_string(), self.model_id.clone()));
        let (status, resp) = self
            .request("POST", "/v1/transcribe", &headers, body)
            .await?;
        let json: serde_json::Value = serde_json::from_slice(&resp)?;
        if status == 200 {
            json["transcription"]
                .as_str()
                .map(String::from)
                .ok_or_else(|| anyhow!("backend response missing transcription"))
        } else {
            // Surface the backend's own error message (shown to the user)
            // rather than the raw HTTP body.
            let msg = json
                .get("detail")
                .and_then(|v| v.as_str())
                .or_else(|| json.get("message").and_then(|v| v.as_str()))
                .unwrap_or("transcription failed");
            bail!("{msg}");
        }
    }
}

fn json_headers() -> Vec<(String, String)> {
    vec![("content-type".to_string(), "application/json".to_string())]
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

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
async fn spawn_systemd_unit(
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
pub(crate) fn hardening_params(backend_dir: &Path, socket_dir: &Path) -> Vec<String> {
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
