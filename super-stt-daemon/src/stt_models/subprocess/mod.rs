// SPDX-License-Identifier: GPL-3.0-only
//! Host for STT backends shipped as sandboxed native subprocesses
//! (experimental — gated behind the `subprocess-backends` feature).
//!
//! [`SubprocessBackend`] provisions a backend's model files (downloading from
//! `HuggingFace` into the per-backend directory), spawns the backend binary
//! into a sandbox, drives the `/v1` contract over a pathname Unix socket, and
//! presents the result through the daemon's [`Transcribe`] trait. The backend
//! itself is fully self-contained and shares no code with the daemon.
//!
//! The sandbox is the one genuinely per-platform part: a hardened
//! `systemd-run --user` transient unit on Linux ([`systemd`]), a
//! `sandbox-exec` profile on macOS ([`sandbox_exec`]). Both expose the same
//! handle — spawn, label, logs, stop — so everything below this line is the
//! same code on both. Their module docs set out what each confines, and
//! where macOS is weaker.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use log::info;
use tokio::net::UnixStream;

use super_stt_shared::utils::audio::{ResampleQuality, resample};

use crate::stt_models::backends::manifest::Manifest;
use crate::stt_models::transcribe::{ModelInfo, ModelInfoData, ModelState, Transcribe};

#[cfg(target_os = "macos")]
mod sandbox_exec;
#[cfg(target_os = "linux")]
mod systemd;

#[cfg(target_os = "macos")]
use sandbox_exec::Sandboxed as Supervisor;
/// The platform's handle to a running, sandboxed backend.
#[cfg(target_os = "linux")]
use systemd::Unit as Supervisor;

#[cfg(target_os = "macos")]
use sandbox_exec::spawn_sandboxed;
#[cfg(target_os = "linux")]
use systemd::spawn_sandboxed;

const SAMPLE_RATE: u32 = 16000;

/// A running, sandboxed subprocess backend usable as a [`Transcribe`] model.
pub struct SubprocessBackend {
    socket: PathBuf,
    /// Handle to the sandbox the backend runs in. Dropping it stops the
    /// backend, which is why teardown needs no `Drop` impl of its own here.
    supervisor: Supervisor,
    model_id: String,
    info: ModelInfoData,
    /// Device label reported by the backend's `/v1/status` (e.g. `"cuda"`).
    device: String,
    /// The `x-stt-secret-*` / `x-stt-option-*` pairs injected on every `/v1`
    /// request, per the contract's request-header section. Resolved from the
    /// user's settings at spawn, like the WASM transport's, and replaced in
    /// place by [`Transcribe::reconfigure`] when those settings change.
    ///
    /// Behind a lock rather than owned outright because the request path holds
    /// only `&self`, and because the alternative to swapping it is reloading
    /// the model — which for a subprocess backend means tearing down the unit
    /// and re-provisioning the weights to change a header.
    context_headers: std::sync::RwLock<Vec<(String, String)>>,
}

impl SubprocessBackend {
    /// Provision the selected model, spawn the sandboxed backend, and load it.
    ///
    /// `backend_dir` holds `backend.toml` and the `entrypoint` binary; model
    /// files are downloaded into `<backend_dir>/<dest>`. `device_pref` is the
    /// resolved accelerator (`"cpu"`, `"cuda"`, `"rocm"`, `"metal"`,
    /// `"vulkan"`), or empty when none resolved, which leaves the backend to
    /// select for itself. `context_headers` are the already-formed
    /// `x-stt-secret-*` / `x-stt-option-*` pairs to inject on every request.
    ///
    /// # Errors
    /// Returns an error if provisioning, spawning, or loading fails.
    pub async fn spawn(
        backend_dir: &Path,
        model_name: &str,
        device_pref: &str,
        tracker: Option<&Arc<crate::download_progress::DownloadProgressTracker>>,
        context_headers: Vec<(String, String)>,
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
        // Route through the shared validated helper so it gets the same
        // traversal/prefix/length guards as the daemon's own sockets, instead
        // of a raw `$XDG_RUNTIME_DIR` join (Tier 2 #7).
        //
        // Keyed by backend directory *and* model, not by model alone: the
        // daemon runs two backend instances at once (the transcription model
        // and the post-processor), and two backends may legitimately serve the
        // same model name. Keyed by model alone, the second spawn's
        // `remove_file` below would unlink the live instance's socket and
        // either teardown would take out the other's.
        let socket_dir = super_stt_shared::validation::secure_runtime_path("backends");
        std::fs::create_dir_all(&socket_dir)?;
        // Canonicalize now that the directory exists. The runtime dir is
        // reached through a symlink on macOS (`/var` -> `/private/var`), and
        // the eight bytes that adds are eight bytes of the `sun_path` budget
        // below — budgeting against the pre-canonical spelling would mint a
        // name the kernel then refuses to bind.
        let socket_dir = std::fs::canonicalize(&socket_dir).unwrap_or(socket_dir);

        let instance = instance_key(backend_dir, model_name, max_instance_key(&socket_dir)?);
        let socket = socket_dir.join(format!("{instance}.sock"));
        let _ = std::fs::remove_file(&socket);

        let binary = backend_dir.join(&manifest.backend.entrypoint);
        anyhow::ensure!(
            binary.exists(),
            "backend binary not found: {}",
            binary.display()
        );

        let supervisor = spawn_sandboxed(
            &instance,
            &binary,
            backend_dir,
            &socket_dir,
            &socket,
            &model.supported_devices,
        )
        .await?;

        let interval = model
            .processing_interval_ms
            .map_or_else(|| Duration::from_secs(2), Duration::from_millis);
        let info = ModelInfoData::new(
            model_name,
            manifest.backend.source.clone(),
            model.multilingual,
            model.is_online(),
            interval,
        );

        let mut backend = Self {
            socket,
            supervisor,
            model_id: model_name.to_string(),
            info,
            device: "unknown".to_string(),
            context_headers: std::sync::RwLock::new(context_headers),
        };

        backend.wait_for_ping(Duration::from_secs(30)).await?;
        backend
            .load(model_name, model.provider.as_deref(), device_pref)
            .await?;
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
                    "backend {} did not start within {timeout:?}.\n{}",
                    self.supervisor.label(),
                    self.unit_logs()
                );
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// `POST /v1/load` then poll `/v1/status` until `ready` (or `error`),
    /// capturing the device label the backend reports.
    async fn load(&mut self, name: &str, provider: Option<&str>, device_pref: &str) -> Result<()> {
        let body = serde_json::to_vec(&load_body(name, provider, device_pref))?;
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

    /// The secret/option pairs to inject on this request.
    ///
    /// Cloned rather than borrowed so the guard is dropped before the socket
    /// round-trip: these are a handful of short strings, and holding a read
    /// guard across a transcription would block a settings write for as long as
    /// the transcription runs.
    fn context_headers(&self) -> Vec<(String, String)> {
        self.context_headers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// One HTTP request over the backend's Unix socket, carrying `headers`
    /// plus the secret/option context every `/v1` request gets.
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
        let context = self.context_headers();
        for (k, v) in headers.iter().chain(&context) {
            builder = builder.header(k.as_str(), v.as_str());
        }
        let req = builder.body(Full::new(Bytes::from(body)))?;

        let resp = sender.send_request(req).await?;
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await?.to_bytes().to_vec();
        Ok((status, bytes))
    }

    /// Capture recent backend logs for diagnostics.
    fn unit_logs(&self) -> String {
        self.supervisor.logs()
    }
}

impl Drop for SubprocessBackend {
    fn drop(&mut self) {
        // Stopping the backend is the supervisor's own `Drop`, which runs
        // when the field below is dropped — synchronously, and idempotently
        // after an awaited `shutdown`. All that is left here is the socket
        // file, which neither supervisor knows about.
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
    /// Swap the injected secret/option pairs. The next `/v1` request carries
    /// them; one already in flight keeps the set it was built with.
    ///
    /// `user_allowed_hosts` is ignored, and there is nothing here to ignore it
    /// with: the unit runs under `PrivateNetwork=yes`, so a subprocess backend
    /// has no egress to authorize and `base_url` means nothing to it.
    fn reconfigure(&self, context: crate::stt_models::transcribe::BackendContext) {
        *self
            .context_headers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = context.headers;
    }

    /// Stop the sandboxed backend asynchronously and remove the socket file.
    /// Called by the daemon before the
    /// [`LoadedModel`](crate::daemon::types::LoadedModel) is dropped — gives
    /// us a real `.await` instead of blocking the runtime in `Drop`. After
    /// this returns, the synchronous `Drop` path is a no-op (the supervisor
    /// records that it already stopped) and stays for crash paths and tests.
    async fn shutdown(&mut self) -> Result<()> {
        self.supervisor.stop().await;
        let _ = std::fs::remove_file(&self.socket);
        Ok(())
    }

    async fn transcribe_audio(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<String> {
        // The daemon owns resampling; backends receive 16 kHz.
        let audio16 = resample(audio, sample_rate, SAMPLE_RATE, ResampleQuality::Fast)?;
        let body = crate::stt_models::v1::build_transcribe_body(&audio16, SAMPLE_RATE, language)?;
        let mut headers = json_headers();
        headers.push(("x-stt-model".to_string(), self.model_id.clone()));
        let (status, resp) = self
            .request("POST", "/v1/transcribe", &headers, body)
            .await?;
        crate::stt_models::v1::parse_transcribe_response(status, &resp)
    }

    async fn process_text(&mut self, text: &str, language: Option<&str>) -> Result<String> {
        let body = crate::stt_models::v1::build_process_body(text, language)?;
        let mut headers = json_headers();
        headers.push(("x-stt-model".to_string(), self.model_id.clone()));
        let (status, resp) = self.request("POST", "/v1/process", &headers, body).await?;
        crate::stt_models::v1::parse_process_response(status, &resp)
    }
}

fn json_headers() -> Vec<(String, String)> {
    vec![("content-type".to_string(), "application/json".to_string())]
}

/// Build the `POST /v1/load` body. `name` is always present; `device` only
/// when the daemon resolved an accelerator to name, and `provider` only when
/// the model's manifest declares one.
///
/// `provider` is a compatibility echo (see [`ModelEntry::provider`]): backends
/// released against the earlier `(name, provider)` identity answer
/// `400 invalid_model` for a load body that omits it, so whatever the manifest
/// declares is forwarded verbatim.
///
/// [`ModelEntry::provider`]: crate::stt_models::backends::manifest::ModelEntry::provider
fn load_body(name: &str, provider: Option<&str>, device_pref: &str) -> serde_json::Value {
    let mut load = serde_json::json!({ "name": name });
    if let Some(provider) = provider {
        load["provider"] = serde_json::json!(provider);
    }
    if !device_pref.is_empty() {
        load["device"] = serde_json::json!(device_pref);
    }
    load
}

/// Hex digits in the disambiguating hash appended to a truncated key.
const DIGEST_LEN: usize = 16;

/// Shortest instance key worth minting: a one-character head, a separator,
/// and the full digest. Below this the key is all hash and the truncation
/// carries no hint of what it names, so a socket directory this deep is
/// reported as an error rather than papered over.
const MIN_INSTANCE_KEY: usize = DIGEST_LEN + 2;

/// Upper bound on an instance key regardless of how much room the socket
/// path leaves.
///
/// Keys are almost always far shorter; this bounds the tail case, since a
/// backend `id` — which names the install directory — may be up to 255 bytes
/// on its own, and a 255-byte file name is unreadable in a log line whether
/// or not it fits.
const MAX_INSTANCE_KEY: usize = 64;

/// Longest instance key that still leaves room for the socket path in
/// `socket_dir`.
///
/// A pathname Unix socket must fit in `sun_path`, terminator included — 108
/// bytes on Linux, **104 on macOS**. Computed from the real directory rather
/// than assumed, because the room left over differs by platform by more than
/// those four bytes: Linux binds under `/run/user/<uid>/stt/backends/`, about
/// 30 bytes, while the macOS per-user runtime directory is
/// `/private/var/folders/<xx>/<28-char hash>/T/stt/backends/` — around 70,
/// leaving less than half as much for the name.
///
/// # Errors
/// When the directory is so deep that not even [`MIN_INSTANCE_KEY`] fits.
/// That is a misconfigured runtime directory, and failing here names it,
/// where binding would fail later with `EINVAL` and name nothing.
fn max_instance_key(socket_dir: &Path) -> Result<usize> {
    const SUFFIX: usize = ".sock".len();
    const SEPARATOR: usize = 1; // the `/` between the directory and the name
    let budget = super_stt_shared::validation::SUN_PATH_MAX
        .saturating_sub(socket_dir.as_os_str().len() + SEPARATOR + SUFFIX + 1);
    if budget < MIN_INSTANCE_KEY {
        bail!(
            "backend socket directory {} is too deep: it leaves {budget} bytes for a socket \
             name, and the shortest usable one is {MIN_INSTANCE_KEY}",
            socket_dir.display()
        );
    }
    Ok(budget.min(MAX_INSTANCE_KEY))
}

/// The name that identifies one running backend instance — its socket file and
/// its sandbox. Derived from the backend's install directory and the model
/// it serves, so the daemon's two concurrent instances (transcription model and
/// post-processor) never collide, including when two backends serve the same
/// model name.
///
/// A key over `max_len` is truncated with a hash of the full value appended,
/// so an over-long backend id yields a short name that is still unique and
/// still the same on every spawn — rather than a socket path the kernel
/// refuses to bind. `max_len` comes from [`max_instance_key`].
fn instance_key(backend_dir: &Path, model_name: &str, max_len: usize) -> String {
    let dir = backend_dir
        .file_name()
        .map_or_else(String::new, |n| sanitize(&n.to_string_lossy()));
    let key = format!("{dir}-{}", sanitize(model_name));
    if key.len() <= max_len {
        return key;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&key, &mut hasher);
    let digest = format!("{:0DIGEST_LEN$x}", std::hash::Hasher::finish(&hasher));
    // `max_len` total: the truncated head, a separator, and the digest.
    let head = &key[..max_len - digest.len() - 1];
    format!("{head}-{digest}")
}

/// Stop backend processes left behind by a previous daemon run.
///
/// Called at daemon startup as defense against a previous daemon that exited
/// without running `Transcribe::shutdown()` (SIGKILL / panic /
/// `std::process::exit` skipping `Drop`). What "left behind" means, and how
/// one is found again, is per-platform — see
/// [`systemd::cleanup_orphan_units`] and [`sandbox_exec::sweep_orphans`].
pub async fn cleanup_orphan_units() {
    #[cfg(target_os = "linux")]
    systemd::cleanup_orphan_units().await;
    #[cfg(target_os = "macos")]
    sandbox_exec::sweep_orphans(&super_stt_shared::validation::secure_runtime_path(
        "backends",
    ))
    .await;
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
