// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::backends::{self, DiscoveredBackend};
use crate::stt_models::transcribe::Transcribe;
use anyhow::{Result, anyhow, bail};
use log::warn;
use super_stt_shared::models::provider::Provider;
use super_stt_shared::models::registry::ModelDefinition;

impl SuperSTTDaemon {
    /// Build a running backend instance for `(name, provider, source)` plus its
    /// resolved definition. Central routing point for all model loading.
    ///
    /// # Errors
    /// Returns an error if no installed backend serves the model, the backend
    /// kind is unsupported in this build, or instantiation fails.
    pub async fn instantiate_backend(
        &self,
        name: &str,
        provider: &Provider,
        source: &str,
        device_pref: &str,
    ) -> Result<(Box<dyn Transcribe>, ModelDefinition)> {
        let (backend, def) = {
            let backends = self.backends.read().await;
            let (b, d) = backends::find_model(&backends, name, provider, source)
                .ok_or_else(|| anyhow!("no installed backend serves {name} via {provider}"))?;
            (b.clone(), d.clone())
        };

        let instance: Box<dyn Transcribe> = match backend.kind.as_str() {
            "wasm" => self.instantiate_wasm(&backend, &def).await?,
            "subprocess" => {
                self.instantiate_subprocess(&backend, name, device_pref)
                    .await?
            }
            other => bail!("backend {} declares unknown kind '{other}'", backend.source),
        };
        Ok((instance, def))
    }

    #[cfg(feature = "wasm-backends")]
    async fn instantiate_wasm(
        &self,
        backend: &DiscoveredBackend,
        def: &ModelDefinition,
    ) -> Result<Box<dyn Transcribe>> {
        use crate::stt_models::transcribe::ModelInfoData;
        let headers = self.backend_headers(backend).await?;
        let component = backend.dir.join(&backend.entrypoint);
        let info = ModelInfoData::new(
            def.name.clone(),
            def.provider.clone(),
            def.source.clone(),
            def.is_multilingual,
            def.is_online(),
            def.processing_interval,
        );
        // Websocket capability is a per-backend flag (every model the backend
        // serves shares it). Read it from the manifest so a ws-capable
        // component is linked against the realtime world.
        let websocket_capability =
            crate::stt_models::backends::manifest::Manifest::load(&backend.dir)?
                .capabilities
                .websocket;
        let inst = crate::stt_models::wasm::WasmBackend::with_info(
            &component,
            backend.allowed_hosts.clone(),
            info,
            headers,
            websocket_capability,
        )?;
        Ok(Box::new(inst))
    }

    #[cfg(not(feature = "wasm-backends"))]
    async fn instantiate_wasm(
        &self,
        backend: &DiscoveredBackend,
        _def: &ModelDefinition,
    ) -> Result<Box<dyn Transcribe>> {
        bail!(
            "backend {} is a WASM backend, unsupported in this build (rebuild with --features wasm-backends)",
            backend.source
        )
    }

    #[cfg(feature = "subprocess-backends")]
    async fn instantiate_subprocess(
        &self,
        backend: &DiscoveredBackend,
        name: &str,
        device_pref: &str,
    ) -> Result<Box<dyn Transcribe>> {
        // Count the files we'll provision so the tracker's denominator is
        // accurate from the first broadcast (vs. growing as we discover more
        // `[[models.files]]` blocks). Empty-files models (cloud-only) skip
        // the tracker entirely — there is nothing to download.
        let manifest = crate::stt_models::backends::manifest::Manifest::load(&backend.dir)?;
        let total_files = manifest
            .models
            .iter()
            .find(|m| m.name == name)
            .map_or(0, |m| m.files.iter().map(|s| s.files.len()).sum::<usize>());

        let tracker = if total_files == 0 {
            None
        } else {
            let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let t = std::sync::Arc::new(
                crate::download_progress::DownloadProgressTracker::new(
                    name.to_string(),
                    total_files,
                    cancelled,
                )
                .with_event_bus(std::sync::Arc::clone(&self.events)),
            );
            // Register so `GET /download_status` returns this tracker and the
            // settings app's progress card lights up. A previous tracker (from
            // a failed load) is cleared first — the manager rejects parallel
            // downloads, but a leftover entry would block this one.
            self.download_manager.clear_download();
            if let Err(e) = self
                .download_manager
                .start_download(std::sync::Arc::clone(&t))
            {
                warn!("could not register download tracker: {e}");
            }
            // Emit the initial state immediately so the UI shows "0%" rather
            // than nothing while the first chunk lands.
            t.broadcast_progress();
            Some(t)
        };

        let result = crate::stt_models::subprocess::SubprocessBackend::spawn(
            &backend.dir,
            name,
            device_pref,
            tracker.as_ref(),
        )
        .await;

        // Whatever happened (success, error, cancel), the tracker has done
        // its job — mark the terminal status and clear the manager so the
        // UI's progress card collapses and the next load can register.
        if let Some(t) = &tracker {
            match &result {
                Ok(_) => t.mark_completed(),
                Err(e) => t.mark_error(&format!("{e:#}")),
            }
            t.broadcast_progress();
            self.download_manager.clear_download();
        }

        Ok(Box::new(result?))
    }

    #[cfg(not(feature = "subprocess-backends"))]
    async fn instantiate_subprocess(
        backend: &DiscoveredBackend,
        _name: &str,
        _device_pref: &str,
    ) -> Result<Box<dyn Transcribe>> {
        bail!(
            "backend {} is a subprocess backend, unsupported in this build (rebuild with --features subprocess-backends)",
            backend.source
        )
    }

    /// Form `x-stt-secret-*` / `x-stt-option-*` headers for a WASM backend.
    ///
    /// Secrets come solely from the generic per-backend keyring store
    /// (`backend:<source>:<name>`) written by the settings app — there is no
    /// legacy `<provider>-api-key` fallback, so the key must be set for this
    /// specific backend. Options use the config override if set, else the
    /// manifest default. A required secret that resolves to nothing is an error.
    #[cfg(feature = "wasm-backends")]
    async fn backend_headers(&self, backend: &DiscoveredBackend) -> Result<Vec<(String, String)>> {
        let mut headers = Vec::new();
        for secret in &backend.secrets {
            let value = crate::keyring::get_backend_secret(&backend.source, &secret.name)
                .map_err(|e| anyhow!(e))?
                .filter(|v| !v.is_empty());
            match value {
                Some(v) => headers.push((format!("x-stt-secret-{}", secret.name), v)),
                // Safety-net error: the settings UI is expected to surface this
                // requirement *before* the user can request a model load. If
                // that pre-flight is bypassed (a UI bug, or a non-UI client),
                // the daemon is the final guard — keep the message short and
                // user-facing rather than naming internals (`secret name`,
                // `backend source`), since the caller already chose this
                // backend.
                None if secret.required => bail!(
                    "{} must be set.",
                    secret.label.as_deref().unwrap_or(&secret.name)
                ),
                None => {}
            }
        }
        let config = self.config.read().await;
        for opt in &backend.options {
            let value = config
                .backend_option(&backend.source, &opt.name)
                .map(str::to_string)
                .or_else(|| opt.default.as_ref().map(ToString::to_string));
            if let Some(v) = value {
                headers.push((format!("x-stt-option-{}", opt.name), v));
            }
        }
        Ok(headers)
    }
}
