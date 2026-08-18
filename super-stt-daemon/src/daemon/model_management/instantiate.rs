// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperSTTDaemon;
use crate::registry::host_detect::Host;
use crate::registry::installed;
use crate::stt_models::ModelDefinition;
use crate::stt_models::backends::{self, DiscoveredBackend};
use crate::stt_models::transcribe::Transcribe;
use anyhow::{Result, anyhow, bail};

impl SuperSTTDaemon {
    /// Build a running backend instance for `(name, source)` plus its
    /// resolved definition. Central routing point for all model loading —
    /// startup, a model switch, a device switch, and a config reload all
    /// funnel through here, so `device_pref` (the user's `cpu`/`gpu`
    /// preference) is resolved into the concrete accelerator right here
    /// rather than by each caller.
    ///
    /// # Errors
    /// Returns an error if no installed backend serves the model, the backend
    /// kind is unsupported in this build, or instantiation fails.
    pub async fn instantiate_backend(
        &self,
        name: &str,
        source: &str,
        device_pref: &str,
    ) -> Result<(Box<dyn Transcribe>, ModelDefinition)> {
        let (backend, def) = {
            let backends = self.backends.read().await;
            let (b, d) = backends::find_model(&backends, name, source)
                .ok_or_else(|| anyhow!("no installed backend serves {name}"))?;
            (b.clone(), d.clone())
        };

        let instance: Box<dyn Transcribe> = match backend.kind.as_str() {
            "wasm" => self.instantiate_wasm(&backend, &def).await?,
            "subprocess" => {
                let resolved = resolve_device_for_backend(device_pref, &backend.dir).await;
                self.instantiate_subprocess(&backend, name, &resolved)
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
        // One snapshot of the user's options for both the headers the component
        // is handed and the egress it is granted: read separately, a config
        // write landing between them would authorize a different endpoint than
        // the one the component is told to use.
        let overrides = self.backend_option_overrides(backend).await?;
        let headers = self.backend_headers(backend, &overrides).await?;
        let component = backend.dir.join(&backend.entrypoint);
        let info = ModelInfoData::new(
            def.name.clone(),
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
        // Egress = the manifest-pinned `allowed_hosts` (fully SSRF-guarded) plus
        // what the user authorized via the `base_url` option, whose `host:port`
        // may be local or private.
        let user_allowed_hosts = Self::base_url_egress_hosts(backend, &overrides);
        let inst = crate::stt_models::wasm::WasmBackend::with_info(
            &component,
            backend.allowed_hosts.clone(),
            user_allowed_hosts,
            info,
            headers,
            websocket_capability,
            def.realtime,
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
        // accurate from the first broadcast. Each `[[models.files]]` entry is
        // one file. Empty-files models (cloud-only) skip the tracker entirely —
        // there is nothing to download.
        let manifest = crate::stt_models::backends::manifest::Manifest::load(&backend.dir)?;
        let total_files = manifest
            .models
            .iter()
            .find(|m| m.name == name)
            .map_or(0, |m| m.files.len());

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
                log::warn!("could not register download tracker: {e}");
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
        &self,
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
    async fn backend_headers(
        &self,
        backend: &DiscoveredBackend,
        overrides: &std::collections::HashMap<String, String>,
    ) -> Result<Vec<(String, String)>> {
        let mut headers = Vec::new();
        for secret in &backend.secrets {
            let value = crate::keyring::get_backend_secret_async(
                backend.source.clone(),
                secret.name.clone(),
            )
            .await
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
        for opt in &backend.options {
            if let Some(v) = resolved_backend_option(overrides, opt) {
                headers.push((format!("x-stt-option-{}", opt.name), v));
            }
        }
        Ok(headers)
    }

    /// Snapshot of the user's option overrides for one backend.
    ///
    /// Taken once per load and shared by header injection and egress
    /// derivation. Resolving them separately let a config write land in
    /// between, so the component could be handed one gateway while a different
    /// one was authorized, and every request would then be refused until the
    /// model was reloaded. Cloned rather than held as a guard: the header path
    /// awaits a keyring round-trip, and a read guard spanning that would block
    /// config writers for its duration.
    ///
    /// `base_url` is canonicalized on the way out (see
    /// [`normalize`](backends::base_url::normalize)). It is the one value the
    /// two paths must read *identically* — one dials it, the other authorizes
    /// what it names — and the component is handed it verbatim, so the rewrite
    /// that keeps the pair consistent is also what spares every backend its own
    /// URL parser. Every other option is passed through exactly as the user set
    /// it: whitespace may carry meaning in a value the daemon does not
    /// interpret.
    ///
    /// The two ways a value can fail are not the same failure. One that is only
    /// whitespace is no value at all: it is dropped, so it neither reaches the
    /// component nor authorizes anything, and the backend falls back to its
    /// built-in endpoint exactly as if nothing were set. One the daemon cannot
    /// read as a URL is a setting the user meant, so it fails the load instead —
    /// dropping it would fall back to that same built-in endpoint and send the
    /// user's audio and credentials to the vendor they had configured their way
    /// out of.
    ///
    /// A value stored for an option this backend does not declare is inert —
    /// nothing injects or authorizes it — so it is left alone rather than
    /// validated.
    ///
    /// # Errors
    /// Returns an error when a declared, non-empty `base_url` yields no host.
    #[cfg(feature = "wasm-backends")]
    async fn backend_option_overrides(
        &self,
        backend: &DiscoveredBackend,
    ) -> Result<std::collections::HashMap<String, String>> {
        let mut overrides: std::collections::HashMap<String, String> = self
            .config
            .read()
            .await
            .backends
            .options
            .get(&backend.source)
            .cloned()
            .unwrap_or_default();
        let name = backends::base_url::OPTION_NAME;
        let Some(opt) = backend.options.iter().find(|o| o.name == name) else {
            return Ok(overrides);
        };
        let Some(raw) = overrides.get(name).cloned() else {
            return Ok(overrides);
        };
        if raw.trim().is_empty() {
            overrides.remove(name);
        } else if let Some(canonical) = backends::base_url::normalize(&raw) {
            // The scheme the daemon chose for a value that named none decides
            // whether the request is encrypted, so an operator asking later why
            // a gateway was reached in the clear needs it on the record.
            if !raw.contains("://") {
                log::info!(
                    "Backend {}: base_url `{}` names no scheme; reading it as `{canonical}`",
                    backend.source,
                    raw.trim()
                );
            }
            overrides.insert(name.to_string(), canonical);
        } else {
            // Shaped like the missing-secret error above: name the setting the
            // user can act on, never the internals.
            bail!(
                "{} is not a valid URL.",
                opt.label.as_deref().unwrap_or(&opt.name)
            );
        }
        Ok(overrides)
    }

    /// What the *user* authorized via a `base_url` option: the `host:port` the
    /// value points at, followed by the bare host.
    ///
    /// `base_url` is the documented convention for a backend's configurable
    /// endpoint (`docs/protocol/backend/config.md`); any backend declaring an
    /// option with that name has the SSRF guard relaxed for that one authority.
    /// The value is read from the config override **only** — never from the
    /// manifest default, which the backend author writes and which therefore
    /// cannot be allowed to widen the sandbox. (A manifest declaring one is
    /// refused at publication and scrubbed at load; this read stands on its own
    /// so the invariant does not depend on either check.) Because the value is
    /// the user's, it may be
    /// loopback or private, e.g. a local gateway.
    ///
    /// The bare host carries no such relaxation; it keeps the gateway's other
    /// ports reachable while they stay public, so no extra port on a local or
    /// private gateway opens up (see
    /// [`check_host_allowed`](crate::stt_models::wasm::host::check_host_allowed)).
    /// Unparseable or unset values contribute nothing.
    ///
    /// Both outcomes are logged. This is the one path that relaxes the sandbox,
    /// so an operator asking later why a backend reached a private address needs
    /// a record of which endpoint was authorized for which backend, and when.
    #[cfg(feature = "wasm-backends")]
    fn base_url_egress_hosts(
        backend: &DiscoveredBackend,
        overrides: &std::collections::HashMap<String, String>,
    ) -> Vec<String> {
        if !backend
            .options
            .iter()
            .any(|o| o.name == backends::base_url::OPTION_NAME)
        {
            return Vec::new();
        }
        let Some(value) = overrides.get(backends::base_url::OPTION_NAME) else {
            return Vec::new();
        };
        let entries = backends::base_url::egress_entries(value);
        // Log the derived authority, never the configured value: the parser
        // discards userinfo, so a URL pasted with credentials in it cannot reach
        // the journal through here.
        match entries.first() {
            Some(endpoint) => log::info!(
                "Backend {}: user-set base_url authorizes egress to {endpoint}, with the SSRF guard relaxed for it",
                backend.source
            ),
            None => log::warn!(
                "Backend {}: base_url is set but names no host the daemon can read; it authorizes nothing and the backend keeps only its manifest egress",
                backend.source
            ),
        }
        entries
    }
}

/// Resolve `device_pref` (`cpu`/`gpu`) into the concrete accelerator handed to
/// a subprocess backend's `POST /v1/load`, per
/// `docs/protocol/backend/contract.md`. Reads the backend directory's install
/// record for the asset's declared accel list, and the host's detected
/// capability — a dual-runtime asset resolves by which the *host* can run,
/// not by which entry the asset lists first.
async fn resolve_device_for_backend(device_pref: &str, backend_dir: &std::path::Path) -> String {
    let installed_accel = installed::read(backend_dir)
        .map(|r| r.selected.accel)
        .unwrap_or_default();
    // Best-effort: detection failure degrades `resolve_accel` to its
    // list-order fallback rather than blocking the load.
    let host = tokio::task::spawn_blocking(crate::registry::host_detect::detect)
        .await
        .ok();
    resolve_accel(device_pref, &installed_accel, host.as_ref())
}

/// Turn the user's `cpu`/`gpu` preference into the accelerator the backend is
/// told to load on.
///
/// The daemon knows which it is — it chose the asset — so it sends the
/// concrete value rather than forwarding the preference. A binary carrying
/// several runtimes needs this to pick; one carrying a single runtime ignores
/// it, since the contract has always been "anything that is not `cpu` means
/// use the accelerator".
///
/// A dual-runtime asset (`accel = ["cuda", "rocm"]`) cannot be resolved by
/// list position: `compat::select` (`registry/compat.rs`) chose this asset
/// because the *host* can run one of its declared families, not because of
/// where that family sits in the list, so resolving here has to ask the same
/// question rather than default to "first declared". `host` is `None` only
/// when detection itself failed, or when none of the declared entries match
/// what it reports (should not happen for an asset `compat::select` already
/// approved, but a stale or hand-edited `installed.json` should still degrade
/// rather than panic) — both fall back to the prior list-order heuristic,
/// since a `gpu` preference stays meaningful to a backend on its own.
pub(crate) fn resolve_accel(
    preference: &str,
    installed_accel: &[String],
    host: Option<&Host>,
) -> String {
    if preference == "cpu" {
        return "cpu".to_string();
    }
    if let Some(host) = host
        && let Some(found) = installed_accel.iter().find(|a| host_can_run(host, a))
    {
        return found.clone();
    }
    installed_accel
        .iter()
        .find(|a| *a != "cpu")
        .cloned()
        .unwrap_or_else(|| preference.to_string())
}

/// Whether the host can run `accel`, mirroring the presence checks
/// `compat::score` (`registry/compat.rs`) gates asset selection on. Full
/// sm/gfx/floor compatibility was already proven when the asset was
/// selected; this only has to tell apart two host-compatible accelerators
/// declared by the same installed asset.
fn host_can_run(host: &Host, accel: &str) -> bool {
    match accel {
        "cuda" => host.cuda.is_some(),
        "rocm" => host.rocm.is_some(),
        "vulkan" => host.vulkan.is_some(),
        _ => false,
    }
}

#[cfg(test)]
mod resolve_accel_tests {
    use super::{Host, resolve_accel};
    use crate::registry::host_detect::{CudaHost, RocmHost, VulkanHost};

    fn nvidia_host() -> Host {
        Host {
            target_triple: "x86_64-unknown-linux-gnu".into(),
            cuda: Some(CudaHost {
                compute_capability: 86,
                runtime_major: 13,
                cudnn_present: true,
            }),
            rocm: None,
            vulkan: None,
        }
    }

    fn amd_host() -> Host {
        Host {
            target_triple: "x86_64-unknown-linux-gnu".into(),
            cuda: None,
            rocm: Some(RocmHost {
                gfx_targets: vec![gpu_probe::GfxTarget::new(11, 0, 0)],
                version: None,
            }),
            vulkan: None,
        }
    }

    fn vulkan_only_host() -> Host {
        Host {
            target_triple: "x86_64-unknown-linux-gnu".into(),
            cuda: None,
            rocm: None,
            vulkan: Some(VulkanHost {
                api_version: gpu_probe::VulkanVersion::new(1, 3, 0),
            }),
        }
    }

    /// The user picks `cpu` or `gpu`; the backend is told which accelerator,
    /// because a build carrying several runtimes has to be able to choose.
    #[test]
    fn a_gpu_preference_resolves_to_the_accel_the_host_can_run() {
        assert_eq!(
            resolve_accel("gpu", &["cuda".into()], Some(&nvidia_host())),
            "cuda"
        );
        assert_eq!(
            resolve_accel("gpu", &["rocm".into()], Some(&amd_host())),
            "rocm"
        );
        assert_eq!(
            resolve_accel("gpu", &["vulkan".into()], Some(&vulkan_only_host())),
            "vulkan"
        );
    }

    #[test]
    fn a_cpu_preference_always_resolves_to_cpu() {
        assert_eq!(
            resolve_accel("cpu", &["cuda".into()], Some(&nvidia_host())),
            "cpu"
        );
        assert_eq!(resolve_accel("cpu", &[], None), "cpu");
    }

    /// The bug this pins: `compat::select` chose this asset because the host
    /// can run one of its declared families, never because of list position.
    /// An asset declaring `["cuda", "rocm"]` must resolve to whichever the
    /// *host* actually has — not to `cuda` just because it was declared
    /// first — or the daemon tells a ROCm-only host to load a runtime it
    /// cannot run.
    #[test]
    fn a_dual_runtime_asset_resolves_by_host_capability_not_list_position() {
        assert_eq!(
            resolve_accel("gpu", &["rocm".into(), "cuda".into()], Some(&nvidia_host())),
            "cuda"
        );
        assert_eq!(
            resolve_accel("gpu", &["cuda".into(), "rocm".into()], Some(&amd_host())),
            "rocm"
        );
    }

    /// Detection failure (`host: None`) degrades to the list-order heuristic
    /// rather than blocking the load — a `gpu` preference stays meaningful to
    /// a backend on its own.
    #[test]
    fn an_unknown_host_falls_back_to_list_order() {
        assert_eq!(
            resolve_accel("gpu", &["cpu".into(), "rocm".into()], None),
            "rocm"
        );
    }

    /// A host that matches none of the asset's declared entries (a stale or
    /// hand-edited record) also degrades to list order rather than erroring.
    #[test]
    fn a_host_matching_nothing_declared_falls_back_to_list_order() {
        assert_eq!(
            resolve_accel("gpu", &["rocm".into()], Some(&nvidia_host())),
            "rocm"
        );
    }

    /// No record — a local-directory import, or an install predating the
    /// record. `gpu` is still meaningful to a backend: everything that is not
    /// `cpu` means "use the accelerator you have".
    #[test]
    fn a_gpu_preference_without_a_record_stays_gpu() {
        assert_eq!(resolve_accel("gpu", &[], None), "gpu");
    }
}

/// The effective value of a backend option — the user's override if set, else
/// the manifest default — as injected into the backend's headers by
/// [`SuperSTTDaemon::backend_headers`].
///
/// Not what authorizes egress: `base_url_egress_hosts` deliberately reads the
/// override alone, because a manifest default is the backend author's value and
/// must not widen the sandbox. The settings-facing read path
/// (`http/v1/backends/options.rs::effective`) resolves the same two sources
/// separately, for display.
#[cfg(feature = "wasm-backends")]
fn resolved_backend_option(
    overrides: &std::collections::HashMap<String, String>,
    opt: &backends::manifest::Opt,
) -> Option<String> {
    overrides
        .get(&opt.name)
        .cloned()
        .or_else(|| opt.default.as_ref().map(ToString::to_string))
}

#[cfg(all(test, feature = "wasm-backends"))]
mod tests {
    use super::SuperSTTDaemon;

    /// Only a user-set `base_url` feeds the egress allowlist, as the endpoint it
    /// names followed by its bare host. A backend declaring no such option, and
    /// one the user has not configured, both contribute nothing — a manifest
    /// value must never widen the sandbox.
    #[tokio::test]
    async fn base_url_egress_hosts_resolves_override_or_default() {
        use crate::daemon::test_fixtures::openai_backend;
        use crate::daemon::types::test_daemon;
        use crate::stt_models::backends::DiscoveredBackend;

        let daemon = test_daemon().await;
        let source = "github.com/super-stt/openai";
        // The manifest default is one `Manifest::parse` would reject; it is here
        // to prove this read does not depend on that rejection.
        let backend = openai_backend(source, Vec::new(), Some("https://api.openai.com"));

        // No override → nothing, even though the option carries a default: a
        // value the backend author wrote must not authorize egress.
        let overrides = daemon
            .backend_option_overrides(&backend)
            .await
            .expect("a valid base_url");
        assert!(SuperSTTDaemon::base_url_egress_hosts(&backend, &overrides).is_empty());

        // Config override pointing at a local gateway → that endpoint, port kept.
        // Only the `host:port` entry carries the relaxation, so the bare host
        // opens no other local port.
        daemon
            .config
            .write()
            .await
            .backends
            .options
            .entry(source.to_string())
            .or_default()
            .insert("base_url".to_string(), "http://localhost:8080".to_string());
        let overrides = daemon
            .backend_option_overrides(&backend)
            .await
            .expect("a valid base_url");
        assert_eq!(
            SuperSTTDaemon::base_url_egress_hosts(&backend, &overrides),
            vec!["localhost:8080", "localhost"]
        );

        // A backend declaring no `base_url` option contributes nothing, even
        // with the override still in config.
        let no_base = DiscoveredBackend {
            options: vec![],
            ..backend
        };
        assert!(SuperSTTDaemon::base_url_egress_hosts(&no_base, &overrides).is_empty());
    }

    /// A `base_url` pasted with surrounding whitespace must reach both paths as
    /// the same string: the component dials the header it is given, and the
    /// daemon authorizes what the value names. A value that is only whitespace
    /// is no value — it must not be injected or authorize anything.
    #[tokio::test]
    async fn whitespace_in_base_url_cannot_split_the_two_paths() {
        use crate::daemon::test_fixtures::openai_backend;
        use crate::daemon::types::test_daemon;
        use crate::stt_models::backends::DiscoveredBackend;

        let daemon = test_daemon().await;
        let source = "github.com/super-stt/openai";
        // No secrets: the assertions run through the real header path, which
        // would otherwise reach the keyring for the fixture's required key.
        let backend = DiscoveredBackend {
            secrets: Vec::new(),
            ..openai_backend(source, Vec::new(), None)
        };

        for (stored, expected_header) in [
            ("  http://10.0.0.5:8080  ", Some("http://10.0.0.5:8080")),
            ("\thttp://10.0.0.5:8080\n", Some("http://10.0.0.5:8080")),
            ("   ", None),
        ] {
            daemon
                .config
                .write()
                .await
                .backends
                .options
                .entry(source.to_string())
                .or_default()
                .insert("base_url".to_string(), stored.to_string());

            let overrides = daemon
                .backend_option_overrides(&backend)
                .await
                .expect("a valid base_url");
            let headers = daemon
                .backend_headers(&backend, &overrides)
                .await
                .expect("headers for a secret-free backend");
            let injected = headers
                .iter()
                .find(|(k, _)| k == "x-stt-option-base_url")
                .map(|(_, v)| v.as_str());
            let egress = SuperSTTDaemon::base_url_egress_hosts(&backend, &overrides);
            assert_eq!(injected, expected_header, "header for {stored:?}");
            match expected_header {
                Some(_) => assert_eq!(egress, vec!["10.0.0.5:8080", "10.0.0.5"]),
                None => assert!(egress.is_empty(), "egress for {stored:?}"),
            }
        }
    }

    /// The component is handed the canonical form, not the string the user
    /// typed. A backend dials this value directly, so the rewrite and the
    /// authorization have to describe one endpoint — and a backend reading it
    /// can split at the first `/` rather than carry a URL parser of its own.
    #[tokio::test]
    async fn the_injected_base_url_is_canonical() {
        use crate::daemon::test_fixtures::openai_backend;
        use crate::daemon::types::test_daemon;
        use crate::stt_models::backends::DiscoveredBackend;

        let daemon = test_daemon().await;
        let source = "github.com/super-stt/openai";
        // No secrets: this drives the real header path, which would otherwise
        // reach the keyring for the fixture's required key.
        let backend = DiscoveredBackend {
            secrets: Vec::new(),
            ..openai_backend(source, Vec::new(), None)
        };
        daemon
            .config
            .write()
            .await
            .backends
            .options
            .entry(source.to_string())
            .or_default()
            .insert(
                "base_url".to_string(),
                "  HTTPS://user:pass@gw.example.com:8443/v1/?k=v  ".to_string(),
            );

        let overrides = daemon
            .backend_option_overrides(&backend)
            .await
            .expect("a valid base_url");
        let headers = daemon
            .backend_headers(&backend, &overrides)
            .await
            .expect("headers for a secret-free backend");
        let injected = headers
            .iter()
            .find(|(k, _)| k == "x-stt-option-base_url")
            .map(|(_, v)| v.as_str());
        assert_eq!(injected, Some("https://gw.example.com:8443/v1"));
        assert_eq!(
            SuperSTTDaemon::base_url_egress_hosts(&backend, &overrides),
            vec!["gw.example.com:8443", "gw.example.com"]
        );
    }

    /// A value the daemon cannot read fails the load rather than being dropped:
    /// dropping it would fall back to the backend's built-in endpoint and send
    /// the user's audio and credentials to the vendor they had configured their
    /// way out of. The error names the setting, not the internals.
    ///
    /// The same stored value is inert for a backend declaring no such option —
    /// nothing injects or authorizes it — so it must not block that load.
    #[tokio::test]
    async fn an_unreadable_base_url_fails_the_load() {
        use crate::daemon::test_fixtures::openai_backend;
        use crate::daemon::types::test_daemon;
        use crate::stt_models::backends::DiscoveredBackend;

        let daemon = test_daemon().await;
        let source = "github.com/super-stt/openai";
        let backend = openai_backend(source, Vec::new(), None);
        daemon
            .config
            .write()
            .await
            .backends
            .options
            .entry(source.to_string())
            .or_default()
            .insert("base_url".to_string(), "http://".to_string());

        let err = daemon
            .backend_option_overrides(&backend)
            .await
            .expect_err("an unreadable base_url fails the load");
        assert!(err.to_string().contains("API base URL"), "{err}");

        let undeclared = DiscoveredBackend {
            options: Vec::new(),
            ..backend
        };
        assert!(
            daemon.backend_option_overrides(&undeclared).await.is_ok(),
            "a value for an undeclared option must not block a load"
        );
    }

    /// Headers and egress must describe one config state. Resolving them
    /// separately let a write land in between, handing the component one
    /// endpoint while a different one was authorized; both now read the same
    /// snapshot, so the pair either sees the write or does not.
    #[tokio::test]
    async fn headers_and_egress_read_the_same_snapshot() {
        use crate::daemon::test_fixtures::openai_backend;
        use crate::daemon::types::test_daemon;
        use crate::stt_models::backends::DiscoveredBackend;

        let daemon = test_daemon().await;
        let source = "github.com/super-stt/openai";
        // No secrets: this drives the real `backend_headers`, which would
        // otherwise reach the keyring for the fixture's required key.
        let backend = DiscoveredBackend {
            secrets: Vec::new(),
            ..openai_backend(source, Vec::new(), None)
        };
        daemon
            .config
            .write()
            .await
            .backends
            .options
            .entry(source.to_string())
            .or_default()
            .insert("base_url".to_string(), "http://10.0.0.5:8080".to_string());

        let overrides = daemon
            .backend_option_overrides(&backend)
            .await
            .expect("a valid base_url");
        // A write landing here reaches neither side, which is the point.
        daemon
            .config
            .write()
            .await
            .backends
            .options
            .entry(source.to_string())
            .or_default()
            .insert("base_url".to_string(), "http://10.0.0.9:8080".to_string());

        // Drive the real header path, not the resolver it happens to call: the
        // regression this pins is `backend_headers` reading config for itself.
        let headers = daemon
            .backend_headers(&backend, &overrides)
            .await
            .expect("headers for a secret-free backend");
        let injected = headers
            .iter()
            .find(|(k, _)| k == "x-stt-option-base_url")
            .map(|(_, v)| v.as_str())
            .expect("base_url is injected from the snapshot");
        let egress = SuperSTTDaemon::base_url_egress_hosts(&backend, &overrides);
        assert_eq!(injected, "http://10.0.0.5:8080");
        assert_eq!(egress, vec!["10.0.0.5:8080", "10.0.0.5"]);
    }
}
