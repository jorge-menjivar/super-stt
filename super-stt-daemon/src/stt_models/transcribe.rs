// SPDX-License-Identifier: GPL-3.0-only
//! Unified traits for STT backends presented to the daemon.
//!
//! Three layered surfaces:
//! - [`ModelInfo`] — static metadata (name, provider, source, capabilities).
//! - [`ModelState`] — runtime state that can change after load (device).
//! - [`Transcribe`] — actual inference.
//!
//! `Transcribe` is a supertrait of `ModelState`, which is a supertrait of
//! `ModelInfo`, so a `Box<dyn Transcribe>` exposes all three. The concrete
//! implementors are the backend hosts ([`WasmBackend`](super::wasm) and
//! [`SubprocessBackend`](super::subprocess)); the daemon never sees model
//! internals.

use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;

use super_stt_shared::models::provider::Provider;

/// Static metadata about a loaded model, built from the discovered backend
/// entry that serves it. Self-contained — there is no static registry.
#[derive(Debug, Clone)]
pub struct ModelInfoData {
    /// Wire-level model name.
    pub name: String,
    /// Engine family + routing class.
    pub provider: Provider,
    /// Repo id of the backend serving this model.
    pub source: String,
    /// Whether the model supports multiple languages.
    pub is_multilingual: bool,
    /// Suggested minimum interval between real-time processing chunks.
    pub processing_interval: Duration,
}

impl ModelInfoData {
    /// Build metadata for a discovered backend model.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        provider: Provider,
        source: impl Into<String>,
        is_multilingual: bool,
        processing_interval: Duration,
    ) -> Self {
        Self {
            name: name.into(),
            provider,
            source: source.into(),
            is_multilingual,
            processing_interval,
        }
    }
}

/// Static metadata accessor surface — implemented by every loaded model.
pub trait ModelInfo: Send + Sync {
    /// The underlying metadata payload.
    fn info(&self) -> &ModelInfoData;

    /// Engine family + routing class.
    fn provider(&self) -> Provider {
        self.info().provider
    }

    /// Wire-level name.
    fn display_name(&self) -> &str {
        &self.info().name
    }

    /// Whether this model supports multiple languages.
    fn is_multilingual(&self) -> bool {
        self.info().is_multilingual
    }

    /// Whether this model sends audio to an external API.
    fn is_online(&self) -> bool {
        matches!(self.info().provider, Provider::Online(_))
    }

    /// Suggested minimum interval between real-time processing chunks.
    fn processing_interval(&self) -> Duration {
        self.info().processing_interval
    }
}

/// Runtime state that may change after the model is loaded — currently the
/// device label the backend reports (e.g. `"cpu"`, `"cuda"`, `"remote"`).
pub trait ModelState: ModelInfo {
    /// Short device label the model runs on, as reported by the backend.
    fn device(&self) -> String;
}

/// Common contract for any STT backend.
///
/// Implementations drive an out-of-tree backend over the `/v1` contract
/// (in-process for WASM, over a Unix socket for subprocesses). The trait is
/// `async` so callers treat both uniformly.
#[async_trait]
pub trait Transcribe: ModelState {
    /// Transcribe audio samples to plain text.
    async fn transcribe_audio(&mut self, audio: &[f32], sample_rate: u32) -> Result<String>;

    /// Run a realtime streaming session, pumping frames between the
    /// consumer and an upstream until the session ends. Default:
    /// unsupported. Only WASM backends serving a `realtime` model override
    /// this.
    #[cfg(feature = "wasm-backends")]
    async fn realtime_session(
        &self,
        transport: crate::stt_models::wasm::ws_host::ConsumerStreamTransport,
    ) -> Result<()> {
        let _ = transport;
        anyhow::bail!("this model does not support realtime streaming")
    }

    /// Release any external resources the backend holds. Default no-op.
    ///
    /// Subprocess backends override this to stop the `systemd-run --user`
    /// unit they spawned at load time; in-process WASM backends have
    /// nothing to release. The daemon calls this *before* dropping the
    /// [`LoadedModel`](crate::daemon::types::LoadedModel) so the cleanup
    /// happens in an async context where blocking is appropriate — the
    /// `Drop` impl is a synchronous safety net for crash paths and tests.
    async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }
}
