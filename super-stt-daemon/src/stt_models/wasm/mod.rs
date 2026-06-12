// SPDX-License-Identifier: GPL-3.0-only
//! Host-side driver for STT backends shipped as `wasi:http` proxy components
//! (experimental — gated behind the `wasm-backends` feature).
//!
//! A [`WasmBackend`] loads a component, drives the `/v1` contract in-process
//! over wasmtime's `wasi:http` host, and presents the result through the
//! daemon's [`Transcribe`] trait. Secrets and options are injected as
//! `x-stt-secret-*` / `x-stt-option-*` request headers; outbound egress is
//! confined to the backend's `allowed_hosts` (see [`host::AllowlistHooks`]).

pub mod host;
pub mod ws_host;

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use http_body_util::BodyExt;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::WasiCtx;
use wasmtime_wasi_http::WasiHttpCtx;
use wasmtime_wasi_http::p2::WasiHttpView;
use wasmtime_wasi_http::p2::bindings::ProxyPre;
use wasmtime_wasi_http::p2::bindings::http::types::{ErrorCode, Scheme};

use super_stt_shared::models::provider::Provider;

use crate::stt_models::transcribe::{ModelInfo, ModelInfoData, ModelState, Transcribe};
use host::{AllowlistHooks, Host};

/// Instantiation-ready component, pre-linked against one of the two worlds a
/// backend can target. Both worlds export `wasi:http/incoming-handler`, so the
/// batch `/v1` path works for either; only `Realtime` additionally exports
/// `ws-server` and imports `super-stt:realtime/ws`.
enum BackendPre {
    /// A plain `wasi:http` proxy backend (batch `/v1` only).
    Http(ProxyPre<Host>),
    /// A websocket-capable backend (batch `/v1` plus realtime `ws-server`).
    Realtime(ws_host::RealtimeBackendPre<Host>),
}

/// A loaded WASM backend component, usable as a [`Transcribe`] model.
pub struct WasmBackend {
    engine: Engine,
    pre: BackendPre,
    allowed_hosts: Vec<String>,
    allow_loopback: bool,
    transcribe_headers: Vec<(String, String)>,
    model_id: String,
    info: ModelInfoData,
}

impl WasmBackend {
    /// Load a component for a discovered model. The transcribe headers are the
    /// already-formed `x-stt-secret-*` / `x-stt-option-*` pairs to inject.
    ///
    /// # Errors
    /// Returns an error if the component cannot be loaded or linked.
    pub fn with_info(
        component_path: &Path,
        allowed_hosts: Vec<String>,
        info: ModelInfoData,
        transcribe_headers: Vec<(String, String)>,
        websocket_capability: bool,
    ) -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config)?;
        let component = Component::from_file(&engine, component_path)
            .map_err(|e| anyhow!("loading component {}: {e}", component_path.display()))?;
        Self::verify_imports(&engine, &component)?;
        let mut linker: Linker<Host> = Linker::new(&engine);
        // Link the full wasi command world (the component's Rust std runtime
        // imports `wasi:cli/environment` etc.) plus http. Capabilities remain
        // gated by the locked-down `WasiCtx` below — no preopened directories
        // and no granted sockets — so the component cannot touch the disk or
        // open raw connections; its only egress is the allowlisted
        // `wasi:http/outgoing-handler`.
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
        // A websocket-capable backend additionally imports
        // `super-stt:realtime/ws` and exports `ws-server`; link the host `ws`
        // impl and pre-instantiate against the realtime world. A plain backend
        // pre-instantiates against the `wasi:http` proxy world unchanged.
        let pre = if websocket_capability {
            ws_host::add_to_linker(&mut linker)?;
            BackendPre::Realtime(ws_host::RealtimeBackendPre::new(
                linker.instantiate_pre(&component)?,
            )?)
        } else {
            BackendPre::Http(ProxyPre::new(linker.instantiate_pre(&component)?)?)
        };
        let model_id = info.name.clone();
        Ok(Self {
            engine,
            pre,
            allowed_hosts,
            allow_loopback: false,
            transcribe_headers,
            model_id,
            info,
        })
    }

    /// Permit this backend's egress to loopback addresses (`127.0.0.1`, `::1`).
    ///
    /// The SSRF guard blocks loopback by default so an untrusted backend can't
    /// reach a service bound to localhost. Enable this ONLY for tests or local
    /// development that point the backend at a mock upstream on loopback —
    /// never for an installed/untrusted backend. Only loopback is relaxed;
    /// link-local, private, and the cloud-metadata endpoint stay blocked.
    #[must_use]
    pub fn permit_loopback_egress(mut self) -> Self {
        self.allow_loopback = true;
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
        transcribe_headers: Vec<(String, String)>,
    ) -> Result<Self> {
        let info = ModelInfoData::new(
            model_id,
            Provider::from("openai"),
            "github.com/super-stt/openai",
            true,
            true,
            Duration::from_secs(1),
        );
        Self::with_info(
            component_path,
            allowed_hosts,
            info,
            transcribe_headers,
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
        transcribe_headers: Vec<(String, String)>,
    ) -> Result<Self> {
        let info = ModelInfoData::new(
            model_id,
            Provider::from("mistral"),
            "github.com/super-stt/mistral",
            true,
            true,
            Duration::from_secs(1),
        );
        Self::with_info(
            component_path,
            allowed_hosts,
            info,
            transcribe_headers,
            true,
        )
    }

    /// Reject a component that imports interfaces a sandboxed backend must not
    /// have. WASM backends may import only the `wasi:cli` / `http` / `io` /
    /// `clocks` / `random` interfaces their Rust runtime and the `/v1`
    /// contract need; importing e.g. `wasi:sockets` or `wasi:filesystem` is
    /// refused, so the only network egress is the allowlisted
    /// `wasi:http/outgoing-handler`.
    fn verify_imports(engine: &Engine, component: &Component) -> Result<()> {
        const ALLOWED: &[&str] = &[
            "wasi:cli/",
            "wasi:http/",
            "wasi:io/",
            "wasi:clocks/",
            "wasi:random/",
            // Websocket-capable backends import the daemon-implemented
            // `super-stt:realtime/ws`; a non-ws backend simply won't import it.
            "super-stt:realtime/",
        ];
        for (name, _) in component.component_type().imports(engine) {
            let interface = name.split('@').next().unwrap_or(name);
            if !ALLOWED.iter().any(|p| interface.starts_with(p)) {
                bail!(
                    "backend imports disallowed interface `{name}`: WASM backends \
                     may not access raw sockets or the filesystem"
                );
            }
        }
        Ok(())
    }

    /// Drive one `/v1` request through the component in-process and return its
    /// `(status, body)`.
    async fn invoke(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Vec<u8>,
    ) -> Result<(u16, Vec<u8>)> {
        let host = Host {
            table: ResourceTable::new(),
            wasi: WasiCtx::builder().build(),
            http: WasiHttpCtx::new(),
            hooks: AllowlistHooks {
                allowed_hosts: self.allowed_hosts.clone(),
                allow_loopback: self.allow_loopback,
            },
        };
        let mut store = Store::new(&self.engine, host);

        let mut builder = hyper::Request::builder()
            .method(method)
            .uri(format!("http://backend.local{path}"));
        for (key, value) in headers {
            builder = builder.header(key.as_str(), value.as_str());
        }
        let request = builder
            .body(
                http_body_util::Full::new(bytes::Bytes::from(body))
                    .map_err(|never: std::convert::Infallible| -> ErrorCode { match never {} }),
            )
            .context("building backend request")?;

        let (tx, rx) = tokio::sync::oneshot::channel();
        let req = store
            .data_mut()
            .http()
            .new_incoming_request(Scheme::Http, request)?;
        let out = store.data_mut().http().new_response_outparam(tx)?;
        // Both worlds export `wasi:http/incoming-handler`, so batch `/v1`
        // works for a realtime backend's non-realtime models too.
        match &self.pre {
            BackendPre::Http(p) => {
                let proxy = p.instantiate_async(&mut store).await?;
                proxy
                    .wasi_http_incoming_handler()
                    .call_handle(&mut store, req, out)
                    .await?;
            }
            BackendPre::Realtime(p) => {
                let inst = p.instantiate_async(&mut store).await?;
                inst.wasi_http_incoming_handler()
                    .call_handle(&mut store, req, out)
                    .await?;
            }
        }

        let response = rx
            .await
            .context("backend produced no response")?
            .map_err(|e| anyhow!("backend transport error: {e:?}"))?;
        let status = response.status().as_u16();
        let collected = response.into_body().collect().await?.to_bytes();
        Ok((status, collected.to_vec()))
    }

    /// `GET /v1/status` — readiness snapshot.
    ///
    /// # Errors
    /// Returns an error if the component cannot be invoked or its response is
    /// not valid JSON.
    pub async fn status(&self) -> Result<serde_json::Value> {
        let (_, body) = self.invoke("GET", "/v1/status", &[], Vec::new()).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    /// `GET /v1/ping` — liveness.
    ///
    /// # Errors
    /// Returns an error if the component cannot be invoked or its response is
    /// not valid JSON.
    pub async fn ping(&self) -> Result<serde_json::Value> {
        let (_, body) = self.invoke("GET", "/v1/ping", &[], Vec::new()).await?;
        Ok(serde_json::from_slice(&body)?)
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
    async fn transcribe_audio(&mut self, audio: &[f32], sample_rate: u32) -> Result<String> {
        let body = serde_json::to_vec(&serde_json::json!({
            "audio_data": audio,
            "sample_rate": sample_rate,
        }))?;
        let mut headers = self.transcribe_headers.clone();
        headers.push(("x-stt-model".to_string(), self.model_id.clone()));
        let (status, resp) = self
            .invoke("POST", "/v1/transcribe", &headers, body)
            .await?;
        let json: serde_json::Value =
            serde_json::from_slice(&resp).context("parsing backend transcribe response")?;
        if status == 200 {
            json["transcription"]
                .as_str()
                .map(String::from)
                .ok_or_else(|| anyhow!("backend response missing transcription"))
        } else {
            // Surface the backend's own error message (it is shown to the
            // user) rather than the raw HTTP body. Prefer a human-readable
            // `detail`, falling back to the machine `message`.
            let msg = json
                .get("detail")
                .and_then(|v| v.as_str())
                .or_else(|| json.get("message").and_then(|v| v.as_str()))
                .unwrap_or("transcription failed");
            bail!("{msg}");
        }
    }

    /// Run one consumer realtime session: instantiate the component and invoke
    /// its `super-stt:realtime/ws-server.handle` export with the daemon-injected
    /// headers and a host-owned consumer stream. Returns when the guest's
    /// handler returns. Only valid for websocket-capable backends.
    ///
    /// # Errors
    /// Returns an error if the backend is not realtime-capable, instantiation
    /// fails, or the guest's handler returns a `ws-error`.
    #[cfg(feature = "wasm-backends")]
    async fn realtime_session(&self, transport: ws_host::ConsumerStreamTransport) -> Result<()> {
        let BackendPre::Realtime(pre) = &self.pre else {
            bail!("backend is not websocket-capable");
        };
        let host = Host {
            table: ResourceTable::new(),
            wasi: WasiCtx::builder().build(),
            http: WasiHttpCtx::new(),
            hooks: AllowlistHooks {
                allowed_hosts: self.allowed_hosts.clone(),
                allow_loopback: self.allow_loopback,
            },
        };
        let mut store = Store::new(&self.engine, host);
        // Inject the same x-stt-* headers a batch call gets, plus the model id.
        let mut headers: Vec<(String, Vec<u8>)> = self
            .transcribe_headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone().into_bytes()))
            .collect();
        headers.push((
            "x-stt-model".to_string(),
            self.model_id.clone().into_bytes(),
        ));
        let consumer = store
            .data_mut()
            .table
            .push(ws_host::ConsumerStreamResource::new(transport))?;
        let inst = pre.instantiate_async(&mut store).await?;
        inst.super_stt_realtime_ws_server()
            .call_handle(&mut store, &headers, consumer)
            .await?
            .map_err(|e| anyhow!("ws-server.handle returned error: {e:?}"))
    }
}
