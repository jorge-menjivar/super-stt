// SPDX-License-Identifier: GPL-3.0-only
//! Host-side driver for STT backends shipped as `wasi:http` proxy components
//! (experimental — gated behind the `wasm-backends` feature).
//!
//! A [`WasmBackend`] is a `super_engine_daemon::wasm::WasmComponent`, shared
//! with Super TTS, presented through the daemon's [`Transcribe`] trait.
//! Secrets and options are injected as `x-stt-secret-*` / `x-stt-option-*`
//! request headers; outbound egress is confined to the backend's
//! `allowed_hosts` plus the endpoint the user authorized through its
//! `base_url` option (see [`host::AllowlistHooks`]).

pub mod host;
pub mod ws_host;

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use super_engine_daemon::wasm::WasmComponent;

use crate::stt_models::transcribe::{ModelInfo, ModelInfoData, ModelState, Transcribe};
use host::AllowlistHooks;

/// The realtime package Super STT's backends were published against before
/// `super-engine:realtime` existed. Those backends import `ws` and export
/// `ws-server` under this name, so the daemon keeps offering it.
const LEGACY_REALTIME_PACKAGES: &[&str] = &["super-stt:realtime@0.1.0"];

/// A loaded WASM backend component, usable as a [`Transcribe`] model.
pub struct WasmBackend {
    component: WasmComponent,
    model_id: String,
    /// Whether the active model is realtime-only (`[[models]] realtime = true`).
    /// When set, the batch `transcribe_audio` path is served by an internal
    /// one-shot realtime session — the model's batch endpoint rejects it.
    realtime: bool,
    info: ModelInfoData,
}

impl WasmBackend {
    /// Load a component for a discovered model. The transcribe headers are the
    /// already-formed `x-stt-secret-*` / `x-stt-option-*` pairs to inject.
    ///
    /// `allowed_hosts` are the backend's manifest-pinned `[network].allowed_hosts`
    /// (SSRF-guarded); `user_allowed_hosts` is what the user authorized via a
    /// `base_url` option, whose `host:port` has the guard relaxed (see
    /// [`host::AllowlistHooks::user_allowed_hosts`]).
    ///
    /// # Errors
    /// Returns an error if the component cannot be loaded or linked.
    pub fn with_info(
        component_path: &Path,
        allowed_hosts: Vec<String>,
        user_allowed_hosts: Vec<String>,
        info: ModelInfoData,
        request_headers: Vec<(String, String)>,
        websocket_capability: bool,
        realtime: bool,
    ) -> Result<Self> {
        let component = WasmComponent::load(
            component_path,
            allowed_hosts,
            user_allowed_hosts,
            request_headers,
            websocket_capability,
            LEGACY_REALTIME_PACKAGES,
        )?;
        let model_id = info.name.clone();
        Ok(Self {
            component,
            model_id,
            realtime,
            info,
        })
    }

    /// The egress policy every invocation of this backend enforces. See
    /// `super_engine_daemon::wasm::WasmComponent::allowlist_hooks`.
    #[must_use]
    pub fn allowlist_hooks(&self) -> AllowlistHooks {
        self.component.allowlist_hooks()
    }

    /// Permit this backend's egress to loopback addresses (`127.0.0.1`, `::1`).
    /// For tests and local development only. See
    /// `super_engine_daemon::wasm::WasmComponent::permit_loopback_egress`.
    #[must_use]
    pub fn permit_loopback_egress(mut self) -> Self {
        self.component = self.component.permit_loopback_egress();
        self
    }

    /// Mark this backend's active model as realtime-only, so batch
    /// `transcribe_audio` is served via an internal realtime session. Test-only
    /// opt-in; production sets this through [`Self::with_info`].
    #[must_use]
    pub fn with_realtime(mut self) -> Self {
        self.realtime = true;
        self
    }

    /// Convenience constructor used by the `OpenAI` test harness: synthesizes
    /// an `OpenAI` model identity from `model_id`.
    ///
    /// # Errors
    /// Returns an error if the component cannot be loaded or linked.
    pub fn new(
        component_path: &Path,
        allowed_hosts: Vec<String>,
        model_id: String,
        request_headers: Vec<(String, String)>,
    ) -> Result<Self> {
        let info = ModelInfoData::new(
            model_id,
            "github.com/super-stt/openai",
            true,
            true,
            Duration::from_secs(1),
        );
        Self::with_info(
            component_path,
            allowed_hosts,
            Vec::new(),
            info,
            request_headers,
            false,
            false,
        )
    }

    /// Like [`Self::new`] but loads the component as a websocket-capable
    /// (realtime) backend. Used by realtime backends (e.g. Mistral) whose
    /// component targets the `realtime-backend` world.
    ///
    /// # Errors
    /// Returns an error if the component cannot be loaded or linked.
    pub fn new_realtime(
        component_path: &Path,
        allowed_hosts: Vec<String>,
        model_id: String,
        request_headers: Vec<(String, String)>,
    ) -> Result<Self> {
        let info = ModelInfoData::new(
            model_id,
            "github.com/super-stt/mistral",
            true,
            true,
            Duration::from_secs(1),
        );
        Self::with_info(
            component_path,
            allowed_hosts,
            Vec::new(),
            info,
            request_headers,
            true,
            false,
        )
    }

    /// The headers every model-bound `/v1` request carries: the configured
    /// `x-stt-*` set plus the active model and a JSON content type. Built per
    /// call because `x-stt-model` is appended to a copy of the stored set —
    /// and because the stored set can change under it (see
    /// [`Transcribe::reconfigure`]).
    fn v1_headers(&self) -> Vec<(String, String)> {
        let mut headers = self.component.request_headers();
        headers.push(("content-type".to_string(), "application/json".to_string()));
        headers.push(("x-stt-model".to_string(), self.model_id.clone()));
        headers
    }

    /// `GET /v1/status` — readiness snapshot.
    ///
    /// # Errors
    /// Returns an error if the component cannot be invoked or its response is
    /// not valid JSON.
    pub async fn status(&self) -> Result<serde_json::Value> {
        self.component.status().await
    }

    /// `GET /v1/ping` — liveness.
    ///
    /// # Errors
    /// Returns an error if the component cannot be invoked or its response is
    /// not valid JSON.
    pub async fn ping(&self) -> Result<serde_json::Value> {
        self.component.ping().await
    }

    /// Serve a one-shot transcription for a realtime-only model by driving an
    /// internal realtime session over the buffered audio. We pre-build `start` +
    /// PCM16 frames + `stop` and feed them from a concurrent task into the
    /// bounded consumer channel while the session drains it, then collect only
    /// the final `done` transcript *after* it returns. The feeder captures no
    /// `self`, so this stays valid even from the synchronous (`block_on`)
    /// `transcribe_audio` call sites.
    #[cfg(feature = "wasm-backends")]
    #[allow(clippy::cast_possible_truncation)] // intentional f32 -> i16 PCM clamp
    async fn transcribe_via_realtime(&self, audio: &[f32], sample_rate: u32) -> Result<String> {
        use ws_host::{CONSUMER_INCOMING_CAPACITY, ConsumerStreamTransport, WsFrame};

        // 16-bit PCM, mono, LE. Mistral's `input_audio_buffer.append` caps a
        // single message at 262144 raw bytes; 16384 samples (32768 bytes) per
        // frame stays well under it.
        const FRAME_SAMPLES: usize = 16384;

        let (incoming_tx, incoming_rx) =
            tokio::sync::mpsc::channel::<WsFrame>(CONSUMER_INCOMING_CAPACITY);
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::unbounded_channel::<WsFrame>();

        // Pre-build the consumer frames: start, the audio as PCM16 binary chunks,
        // then stop. The guest breaks on `stop`, so no further consumer recv.
        let mut frames = Vec::with_capacity(audio.len() / FRAME_SAMPLES + 2);
        frames.push(WsFrame::Text(format!(
            "{{\"type\":\"start\",\"sample_rate\":{sample_rate}}}"
        )));
        for chunk in audio.chunks(FRAME_SAMPLES) {
            let mut pcm = Vec::with_capacity(chunk.len() * 2);
            for &s in chunk {
                let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
                pcm.extend_from_slice(&v.to_le_bytes());
            }
            frames.push(WsFrame::Binary(pcm));
        }
        frames.push(WsFrame::Text("{\"type\":\"stop\"}".to_string()));

        // `incoming` is now bounded (audit 2 Tier 1 #7), so feed it from a task
        // that runs concurrently with the session draining it — a synchronous
        // pre-load would deadlock once the frame count exceeds the capacity. The
        // feeder captures no `self`, so it's valid even from the sync `block_on`
        // `transcribe_audio` call sites. Errors still surface through the session
        // result below, not the feeder.
        let feeder = tokio::spawn(async move {
            for frame in frames {
                if incoming_tx.send(frame).await.is_err() {
                    break; // session ended / dropped the receiver early
                }
            }
        });

        let transport = ConsumerStreamTransport {
            incoming: incoming_rx,
            outgoing: outgoing_tx,
        };
        self.realtime_session(transport).await?;
        // Session returned → the guest consumed every frame (or aborted); reap
        // the feeder so it can't outlive this call.
        let _ = feeder.await;

        // The session has returned, so every frame the guest emitted (previews
        // plus the terminal `done`/`error`) is buffered. Return only the final
        // transcription.
        let mut done: Option<String> = None;
        while let Ok(frame) = outgoing_rx.try_recv() {
            let WsFrame::Text(s) = frame else { continue };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else {
                continue;
            };
            match v.get("type").and_then(serde_json::Value::as_str) {
                Some("done") => {
                    done = v
                        .get("transcription")
                        .and_then(serde_json::Value::as_str)
                        .map(String::from);
                }
                Some("error") => {
                    let msg = v
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("realtime transcription failed");
                    bail!("{msg}");
                }
                _ => {} // ignore previews / unknown frames
            }
        }
        done.ok_or_else(|| anyhow!("realtime session produced no final transcription"))
    }
}

impl ModelInfo for WasmBackend {
    fn info(&self) -> &ModelInfoData {
        &self.info
    }
}

impl ModelState for WasmBackend {
    /// WASM backends front a remote API; they have no local compute device.
    fn device(&self) -> String {
        "remote".to_string()
    }
}

#[async_trait]
impl Transcribe for WasmBackend {
    /// Swap the injected secret/option pairs and the egress the user's
    /// `base_url` authorizes, together, because they came from one snapshot of
    /// the settings and disagreeing about the endpoint is exactly the failure
    /// [`BackendContext`](crate::stt_models::transcribe::BackendContext) exists
    /// to prevent. See `super_engine_daemon::wasm::WasmComponent::reconfigure`.
    fn reconfigure(&self, context: crate::stt_models::transcribe::BackendContext) {
        self.component
            .reconfigure(context.headers, context.user_allowed_hosts);
    }

    async fn transcribe_audio(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<String> {
        // A realtime-only model is rejected by the batch endpoint, so serve the
        // regular `/v1/transcribe` request through an internal one-shot realtime
        // session and return only the final transcript.
        if self.realtime {
            return self.transcribe_via_realtime(audio, sample_rate).await;
        }
        let body = crate::stt_models::v1::build_transcribe_body(audio, sample_rate, language)?;
        let headers = self.v1_headers();
        let (status, resp) = self
            .component
            .invoke("POST", "/v1/transcribe", &headers, body)
            .await?;
        crate::stt_models::v1::parse_transcribe_response(status, &resp)
    }

    async fn process_text(&mut self, text: &str, language: Option<&str>) -> Result<String> {
        let body = crate::stt_models::v1::build_process_body(text, language)?;
        let headers = self.v1_headers();
        let (status, resp) = self
            .component
            .invoke("POST", "/v1/process", &headers, body)
            .await?;
        crate::stt_models::v1::parse_process_response(status, &resp)
    }

    /// Run one consumer realtime session: the backend's `ws-server.handle`,
    /// with the same `x-stt-*` headers a batch call gets plus the model id.
    ///
    /// # Errors
    /// Returns an error if the backend is not realtime-capable, instantiation
    /// fails, or the guest's handler returns a `ws-error`.
    #[cfg(feature = "wasm-backends")]
    async fn realtime_session(&self, transport: ws_host::ConsumerStreamTransport) -> Result<()> {
        let mut headers: Vec<(String, Vec<u8>)> = self
            .component
            .request_headers()
            .into_iter()
            .map(|(k, v)| (k, v.into_bytes()))
            .collect();
        headers.push((
            "x-stt-model".to_string(),
            self.model_id.clone().into_bytes(),
        ));
        self.component.realtime_session(headers, transport).await
    }
}
