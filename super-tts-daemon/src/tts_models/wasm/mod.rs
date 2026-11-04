// SPDX-License-Identifier: GPL-3.0-only
//! Host-side driver for TTS backends shipped as `wasi:http` proxy components
//! (experimental — gated behind the `wasm-backends` feature).
//!
//! A [`WasmBackend`] is a `super_engine_daemon::wasm::WasmComponent`, shared
//! with Super STT, presented through the daemon's [`Synthesize`] trait.
//! Secrets and options are injected as `x-tts-secret-*` / `x-tts-option-*`
//! request headers; outbound egress is confined to the backend's
//! `allowed_hosts` plus the endpoint the user authorized through its
//! `base_url` option (see [`host::AllowlistHooks`]).

pub mod host;
pub mod ws_host;

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use http_body_util::BodyExt;
use super_engine_daemon::wasm::WasmComponent;

use crate::tts_models::synthesize::{ModelInfo, ModelInfoData, ModelState, Synthesize};
use host::AllowlistHooks;

/// A loaded WASM backend component, usable as a [`Synthesize`] model.
pub struct WasmBackend {
    component: WasmComponent,
    model_id: String,
    /// Whether the active model is realtime-only (`[[models]] realtime = true`),
    /// i.e. served over the `ws.connect` / `ws-server.handle` halves of the
    /// realtime WIT rather than the batch `/v1/synthesize` endpoint.
    realtime: bool,
    info: ModelInfoData,
}

impl WasmBackend {
    /// Load a component for a discovered model. The request headers are the
    /// already-formed `x-tts-secret-*` / `x-tts-option-*` pairs to inject.
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
            // Super TTS published no realtime backend under a name of its
            // own, so only `super-engine:realtime` is offered.
            &[],
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

    /// Mark this backend's active model as realtime-only. Test-only opt-in;
    /// production sets this through [`Self::with_info`].
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
            "github.com/super-tts/openai",
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
    /// (realtime) backend.
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
            "github.com/super-tts/mistral",
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

    /// `POST /v1/synthesize` — drive one synthesis and feed `sink` as frames
    /// decode.
    ///
    /// Unlike [`Self::invoke`], the guest call and the body read run
    /// **concurrently**. A `wasi:http` guest hands over its response head
    /// (`ResponseOutparam::set`) before it starts writing the body, so awaiting
    /// the guest to completion first — which is what `invoke` does, correctly,
    /// for the small JSON routes — would buffer the whole utterance and make
    /// time-to-first-audio equal total synthesis time. Joining the two lets the
    /// daemon start playing while the component is still producing.
    ///
    /// # Errors
    /// Returns an error if the component cannot be invoked, the response is not
    /// `200`, the headers do not describe usable audio, or the frame stream is
    /// malformed.
    pub async fn synthesize(
        &self,
        req: &crate::tts_models::v1::SynthesizeRequest<'_>,
        sink: &mut (dyn crate::tts_models::v1::SynthesisSink + Send),
    ) -> Result<()> {
        let body = crate::tts_models::v1::build_synthesize_body(req)?;
        let mut headers = self.component.request_headers();
        headers.push(("content-type".to_string(), "application/json".to_string()));
        headers.push(("x-tts-model".to_string(), self.model_id.clone()));
        self.component
            .invoke_streaming(
                "POST",
                "/v1/synthesize",
                &headers,
                body,
                |response| async move {
                    let status = response.status().as_u16();
                    let (parts, body) = response.into_parts();
                    if status != 200 {
                        let collected = body.collect().await?.to_bytes();
                        return Err(crate::tts_models::v1::synthesize_error(status, &collected));
                    }
                    crate::tts_models::v1::pump_synthesis(&parts.headers, body, sink).await
                },
            )
            .await
    }

    /// `POST /v1/voices` — hand the backend one cloned voice's reference
    /// audio.
    ///
    /// # Errors
    /// Returns an error if the component cannot be invoked or refuses the
    /// clip.
    pub async fn register_voice(
        &self,
        req: &crate::tts_models::v1::RegisterVoiceRequest<'_>,
    ) -> Result<()> {
        let body = crate::tts_models::v1::build_register_voice_body(req)?;
        let (status, resp) = self
            .component
            .invoke("POST", "/v1/voices", &self.voice_headers(), body)
            .await?;
        if (200..300).contains(&status) {
            return Ok(());
        }
        Err(crate::tts_models::v1::voice_error(status, &resp))
    }

    /// `DELETE /v1/voices/{voice}` — drop a registered cloned voice. A `404`
    /// is success: the goal is that the backend not hold the voice.
    ///
    /// # Errors
    /// Returns an error if the component cannot be invoked or refuses.
    pub async fn unregister_voice(&self, voice: &str) -> Result<()> {
        let path = format!("/v1/voices/{}", urlencoding::encode(voice));
        let (status, resp) = self
            .component
            .invoke("DELETE", &path, &self.voice_headers(), Vec::new())
            .await?;
        if status == 404 || (200..300).contains(&status) {
            return Ok(());
        }
        Err(crate::tts_models::v1::voice_error(status, &resp))
    }

    /// Headers for the voice routes: whatever the backend's configuration
    /// injects, plus the model the voice is being registered against — a
    /// backend serving several models cannot assume one embedding shape.
    fn voice_headers(&self) -> Vec<(String, String)> {
        let mut headers = self.component.request_headers();
        headers.push(("content-type".to_string(), "application/json".to_string()));
        headers.push(("x-tts-model".to_string(), self.model_id.clone()));
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
impl Synthesize for WasmBackend {
    /// Swap the injected secret/option pairs and the egress the user's
    /// `base_url` authorizes, together, because they came from one snapshot of
    /// the settings and disagreeing about the endpoint is exactly the failure
    /// [`BackendContext`](crate::tts_models::synthesize::BackendContext) exists
    /// to prevent. See `super_engine_daemon::wasm::WasmComponent::reconfigure`.
    fn reconfigure(&self, context: crate::tts_models::synthesize::BackendContext) {
        self.component
            .reconfigure(context.headers, context.user_allowed_hosts);
    }

    /// Forward to the inherent streaming implementation.
    async fn synthesize(
        &self,
        request: &crate::tts_models::v1::SynthesizeRequest<'_>,
        sink: &mut (dyn crate::tts_models::v1::SynthesisSink + Send),
    ) -> Result<()> {
        Self::synthesize(self, request, sink).await
    }

    async fn register_voice(
        &self,
        request: &crate::tts_models::v1::RegisterVoiceRequest<'_>,
    ) -> Result<()> {
        Self::register_voice(self, request).await
    }

    async fn unregister_voice(&self, voice: &str) -> Result<()> {
        Self::unregister_voice(self, voice).await
    }

    /// Run one consumer realtime session: the backend's `ws-server.handle`,
    /// with the same `x-tts-*` headers a batch call gets plus the model id.
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
            "x-tts-model".to_string(),
            self.model_id.clone().into_bytes(),
        ));
        self.component.realtime_session(headers, transport).await
    }
}
