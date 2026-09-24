// SPDX-License-Identifier: GPL-3.0-only
//! Host for STT backends shipped as sandboxed native subprocesses
//! (experimental — gated behind the `subprocess-backends` feature).
//!
//! [`SubprocessBackend`] provisions a backend's model files (downloading from
//! `HuggingFace` into the per-backend directory), has
//! [`super_engine_daemon::subprocess`] spawn the backend binary into a sandbox
//! and load the model, and presents the result through the daemon's
//! [`Transcribe`] trait. The sandbox, the socket and the `/v1` handshake are
//! the engine's; what is here is what a transcription backend is asked. The
//! backend itself is fully self-contained and shares no code with the daemon.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use log::info;
use super_engine_daemon::subprocess::{self as engine, Launch, json_headers};

use super_stt_shared::utils::audio::{ResampleQuality, resample};

use crate::stt_models::backends::manifest::Manifest;
use crate::stt_models::transcribe::{ModelInfo, ModelInfoData, ModelState, Transcribe};

const SAMPLE_RATE: u32 = 16000;

/// A running, sandboxed subprocess backend usable as a [`Transcribe`] model.
pub struct SubprocessBackend {
    backend: engine::SubprocessBackend,
    model_id: String,
    info: ModelInfoData,
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

        let backend = engine::SubprocessBackend::spawn(
            &super_stt_shared::SUPER_STT,
            Launch {
                backend_dir,
                entrypoint: &manifest.backend.entrypoint,
                model: model_name,
                provider: model.provider.as_deref(),
                devices: &model.supported_devices,
                device_pref,
                context_headers,
            },
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

        Ok(Self {
            backend,
            model_id: model_name.to_string(),
            info,
        })
    }

    /// The headers a model request carries: the JSON body, and which model
    /// it is for.
    fn model_headers(&self) -> Vec<(String, String)> {
        let mut headers = json_headers();
        headers.push(("x-stt-model".to_string(), self.model_id.clone()));
        headers
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
        self.backend.device().to_string()
    }
}

#[async_trait]
impl Transcribe for SubprocessBackend {
    /// Swap the injected secret/option pairs. The next `/v1` request carries
    /// them; one already in flight keeps the set it was built with.
    ///
    /// `user_allowed_hosts` is ignored, and there is nothing here to ignore it
    /// with: the sandbox gives a subprocess backend no network, so it has no
    /// egress to authorize and `base_url` means nothing to it.
    fn reconfigure(&self, context: crate::stt_models::transcribe::BackendContext) {
        self.backend.set_context_headers(context.headers);
    }

    /// Stop the sandboxed backend asynchronously and remove the socket file.
    /// Called by the daemon before the
    /// [`LoadedModel`](crate::daemon::types::LoadedModel) is dropped — gives
    /// us a real `.await` instead of blocking the runtime in `Drop`. After
    /// this returns, the synchronous `Drop` path is a no-op and stays for
    /// crash paths and tests.
    async fn shutdown(&mut self) -> Result<()> {
        self.backend.shutdown().await;
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
        let (status, resp) = self
            .backend
            .request("POST", "/v1/transcribe", &self.model_headers(), body)
            .await?;
        crate::stt_models::v1::parse_transcribe_response(status, &resp)
    }

    async fn process_text(&mut self, text: &str, language: Option<&str>) -> Result<String> {
        let body = crate::stt_models::v1::build_process_body(text, language)?;
        let (status, resp) = self
            .backend
            .request("POST", "/v1/process", &self.model_headers(), body)
            .await?;
        crate::stt_models::v1::parse_process_response(status, &resp)
    }
}

/// Stop backends left behind by a previous daemon run. See
/// [`super_engine_daemon::subprocess::cleanup_orphans`].
pub async fn cleanup_orphan_units() {
    engine::cleanup_orphans(&super_stt_shared::SUPER_STT).await;
}
