// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperTTSDaemon;
use crate::registry::host_detect::Host;
use crate::registry::installed;
use crate::tts_models::ModelDefinition;
use crate::tts_models::backends::{self, DiscoveredBackend};
use crate::tts_models::synthesize::Synthesize;
use anyhow::{Result, anyhow, bail};
use super_engine_daemon::devices::resolve_accel;

impl SuperTTSDaemon {
    /// Build a running backend instance for `(name, source)` plus its
    /// resolved definition. Central routing point for all model loading —
    /// startup, a model switch, a device switch, and a config reload all
    /// funnel through here, so `device_pref` (the user's `cpu`/`gpu`
    /// preference) is resolved into the concrete accelerator right here
    /// rather than by each caller.
    ///
    /// Being the one place every load passes through, it is also where loads
    /// are serialized. A load takes minutes and every caller empties the model
    /// slot before starting, so without this two of them overlap: the startup
    /// auto-load and a user's pick would both spawn a backend, put two models
    /// on one GPU, and compile kernels into the same cache at once. The gate is
    /// taken here rather than in each caller because there is no caller that
    /// should be exempt, and one that forgot would look exactly like this bug.
    ///
    /// A request that is superseded while it waits gives up instead of loading
    /// a model nobody is asking for any more. That is an error rather than a
    /// silent return so the caller's log says what happened — the way this
    /// failed before was by saying nothing at all.
    ///
    /// # Errors
    /// Returns an error if a newer load superseded this one while it waited,
    /// no installed backend serves the model, the backend kind is unsupported
    /// in this build, or instantiation fails.
    pub async fn instantiate_backend(
        &self,
        name: &str,
        source: &str,
        device_pref: &str,
    ) -> Result<(Box<dyn Synthesize>, ModelDefinition)> {
        // Claimed before queueing, so waiting here is what reveals a newer
        // request rather than hiding it.
        let ticket = self.loading.ticket();
        let Some(_gate) = self.loading.enter(ticket).await else {
            bail!("loading {name} was superseded by a newer model load");
        };

        let (backend, def) = {
            let backends = self.backends.read().await;
            let (b, d) = backends::find_model(&backends, name, source)
                .ok_or_else(|| anyhow!("no installed backend serves {name}"))?;
            (b.clone(), d.clone())
        };

        let instance: Box<dyn Synthesize> = match backend.kind.as_str() {
            "wasm" => self.instantiate_wasm(&backend, &def).await?,
            "subprocess" => {
                // One probe for the two questions this load asks of the
                // machine: which accelerator the installed asset resolves to,
                // and which per-architecture model files to download.
                let host = tokio::task::spawn_blocking(crate::registry::host_detect::detect)
                    .await
                    .ok();
                let resolved = resolve_device_for_backend(device_pref, &backend.dir, host.as_ref());
                self.instantiate_subprocess(&backend, name, &resolved, host)
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
    ) -> Result<Box<dyn Synthesize>> {
        use crate::tts_models::synthesize::ModelInfoData;
        let context = self.backend_context(backend).await?;
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
            crate::tts_models::backends::manifest::Manifest::load(&backend.dir)?
                .capabilities
                .websocket;
        // Egress = the manifest-pinned `allowed_hosts` (fully SSRF-guarded) plus
        // what the user authorized via the `base_url` option, whose `host:port`
        // may be local or private.
        let inst = crate::tts_models::wasm::WasmBackend::with_info(
            &component,
            backend.allowed_hosts.clone(),
            context.user_allowed_hosts,
            info,
            context.headers,
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
    ) -> Result<Box<dyn Synthesize>> {
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
        host: Option<Host>,
    ) -> Result<Box<dyn Synthesize>> {
        // The same secret/option headers the WASM transport is handed: a
        // subprocess backend reads its `[[options]]` off the request too.
        // Resolved before the download tracker exists, so a missing required
        // secret fails the load without leaving a progress card behind.
        let headers = self.backend_context(backend).await?.headers;

        // A failed probe is not a reason to download nothing: a machine with
        // no accelerator is exactly what an unprobeable one looks like from
        // here, so unconditional and `cpu` variants still resolve, and a
        // destination that has only GPU variants fails with a reason naming
        // the empty probe rather than silently fetching a file this host
        // cannot use.
        let host = host.unwrap_or_else(|| Host {
            target_triple: String::new(),
            cuda: None,
            rocm: None,
            vulkan: None,
            metal: None,
        });

        // Count the files we'll provision so the tracker's denominator is
        // accurate from the first broadcast. Each `[[models.files]]` entry is
        // one file, less the per-architecture variants this host does not take
        // — `SubprocessBackend::spawn` resolves the same list the same way, so
        // the card counts exactly what arrives. Empty-files models (cloud-only)
        // skip the tracker entirely: there is nothing to download.
        let manifest = crate::tts_models::backends::manifest::Manifest::load(&backend.dir)?;
        let total_files = match manifest.models.iter().find(|m| m.name == name) {
            Some(model) => crate::registry::compat::select_files(&host, model)
                .map_err(|reason| anyhow!("provisioning {name}: {reason}"))?
                .len(),
            None => 0,
        };

        let tracker = if total_files == 0 {
            None
        } else {
            let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let t = std::sync::Arc::new(
                crate::download_progress::DownloadProgressTracker::new(
                    name.to_string(),
                    (),
                    total_files,
                    cancelled,
                )
                .with_event_bus(std::sync::Arc::clone(&self.events)),
            );
            // Register so `GET /download_status` returns this tracker and the
            // settings app's progress card lights up. A previous tracker (from
            // a failed load) is cleared first — the manager rejects parallel
            // downloads, but a leftover entry would block this one.
            self.download_manager.clear_download(());
            if let Err(e) = self
                .download_manager
                .start_download(std::sync::Arc::clone(&t))
            {
                log::warn!("could not register download tracker: {e}");
            }
            // Emit the initial state immediately so the UI has something to
            // show while the first file is checked. The tracker opens in its
            // `verifying` phase, so a load whose files are all present reports
            // exactly that and never claims a download.
            t.broadcast_progress();
            Some(t)
        };

        let result = crate::tts_models::subprocess::SubprocessBackend::spawn(
            &backend.dir,
            name,
            device_pref,
            &host,
            tracker.as_ref(),
            headers,
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
            self.download_manager.clear_download(());
        }

        Ok(Box::new(result?))
    }

    #[cfg(not(feature = "subprocess-backends"))]
    async fn instantiate_subprocess(
        &self,
        backend: &DiscoveredBackend,
        _name: &str,
        _device_pref: &str,
        _host: Option<Host>,
    ) -> Result<Box<dyn Synthesize>> {
        bail!(
            "backend {} is a subprocess backend, unsupported in this build (rebuild with --features subprocess-backends)",
            backend.source
        )
    }

    /// Everything the daemon resolves from the user's settings for one
    /// backend, as the one bundle both a load and a settings write hand over.
    ///
    /// The single entry point matters more than the saving: the headers and the
    /// egress list must come from one snapshot of the options (see
    /// [`BackendContext`](crate::tts_models::synthesize::BackendContext)), and
    /// having two callers assemble that pair for themselves is how they would
    /// come to disagree. A load builds an instance around it; a settings write
    /// hands the running instance a new one.
    ///
    /// # Errors
    /// Returns an error if a required secret is unset or a declared `base_url`
    /// names no host.
    #[cfg(any(feature = "wasm-backends", feature = "subprocess-backends"))]
    pub(crate) async fn backend_context(
        &self,
        backend: &DiscoveredBackend,
    ) -> Result<crate::tts_models::synthesize::BackendContext> {
        let overrides = self.backend_option_overrides(backend).await?;
        let headers = self.backend_headers(backend, &overrides).await?;
        // Only the WASM transport can dial anything; a subprocess backend runs
        // under `PrivateNetwork=yes`, so there is no egress for a `base_url` to
        // authorize and nothing to derive.
        #[cfg(feature = "wasm-backends")]
        let user_allowed_hosts = Self::base_url_egress_hosts(backend, &overrides);
        #[cfg(not(feature = "wasm-backends"))]
        let user_allowed_hosts = Vec::new();
        Ok(crate::tts_models::synthesize::BackendContext {
            headers,
            user_allowed_hosts,
        })
    }

    /// Form the `x-tts-secret-*` / `x-tts-option-*` headers a backend is
    /// handed on every `/v1` request, whichever transport carries it.
    ///
    /// Secrets come solely from the generic per-backend keyring store
    /// (`backend:<source>:<name>`) written by the settings app — there is no
    /// legacy `<provider>-api-key` fallback, so the key must be set for this
    /// specific backend. Options use the config override if set, else the
    /// manifest default. A required secret that resolves to nothing is an error.
    #[cfg(any(feature = "wasm-backends", feature = "subprocess-backends"))]
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
                Some(v) => headers.push((format!("x-tts-secret-{}", secret.name), v)),
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
                headers.push((format!("x-tts-option-{}", opt.name), v));
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
    #[cfg(any(feature = "wasm-backends", feature = "subprocess-backends"))]
    async fn backend_option_overrides(
        &self,
        backend: &DiscoveredBackend,
    ) -> Result<std::collections::HashMap<String, String>> {
        #[allow(unused_mut)] // mutated only by the canonicalization below
        let mut overrides: std::collections::HashMap<String, String> = self
            .config
            .read()
            .await
            .backends
            .options
            .get(&backend.source)
            .cloned()
            .unwrap_or_default();
        // `base_url` is an endpoint to dial, and only the WASM transport can
        // dial anything — a subprocess backend has no network. Reading it
        // shares the WASM host's URL code, so a build without that transport
        // passes the value through as set, which costs nothing for the only
        // backends such a build can run.
        #[cfg(feature = "wasm-backends")]
        super_engine_daemon::wasm::base_url::canonicalize_override(backend, &mut overrides)?;
        Ok(overrides)
    }

    /// What the *user* authorized via a `base_url` option. See
    /// `super_engine_daemon::wasm::base_url::egress_hosts`.
    #[cfg(feature = "wasm-backends")]
    fn base_url_egress_hosts(
        backend: &DiscoveredBackend,
        overrides: &std::collections::HashMap<String, String>,
    ) -> Vec<String> {
        super_engine_daemon::wasm::base_url::egress_hosts(backend, overrides)
    }
}

/// Resolve `device_pref` (`cpu`/`gpu`) into the concrete accelerator handed to
/// a subprocess backend's `POST /v1/load`, per
/// `docs/protocol/backend/contract.md`. Reads the backend directory's install
/// record for the asset's declared accel list, and the host's detected
/// capability — a dual-runtime asset resolves by which the *host* can run,
/// not by which entry the asset lists first.
///
/// `host` is `None` when detection itself failed, which degrades `resolve_accel`
/// to its list-order fallback rather than blocking the load.
fn resolve_device_for_backend(
    device_pref: &str,
    backend_dir: &std::path::Path,
    host: Option<&Host>,
) -> String {
    let installed_accel = installed::read(backend_dir)
        .map(|r| r.selected.accel)
        .unwrap_or_default();
    resolve_accel(device_pref, &installed_accel, host)
}

/// The effective value of a backend option — the user's override if set, else
/// the manifest default — as injected into the backend's headers by
/// [`SuperTTSDaemon::backend_headers`].
///
/// Not what authorizes egress: `base_url_egress_hosts` deliberately reads the
/// override alone, because a manifest default is the backend author's value and
/// must not widen the sandbox. The settings-facing read path
/// (`http/v1/backends/options.rs::effective`) resolves the same two sources
/// separately, for display.
#[cfg(any(feature = "wasm-backends", feature = "subprocess-backends"))]
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
    use super::SuperTTSDaemon;

    /// Only a user-set `base_url` feeds the egress allowlist, as the endpoint it
    /// names followed by its bare host. A backend declaring no such option, and
    /// one the user has not configured, both contribute nothing — a manifest
    /// value must never widen the sandbox.
    #[tokio::test]
    async fn base_url_egress_hosts_resolves_override_or_default() {
        use crate::daemon::test_fixtures::openai_backend;
        use crate::daemon::types::test_daemon;
        use crate::tts_models::backends::DiscoveredBackend;

        let daemon = test_daemon().await;
        let source = "github.com/super-tts/openai";
        // The manifest default is one `Manifest::parse` would reject; it is here
        // to prove this read does not depend on that rejection.
        let backend = openai_backend(source, Vec::new(), Some("https://api.openai.com"));

        // No override → nothing, even though the option carries a default: a
        // value the backend author wrote must not authorize egress.
        let overrides = daemon
            .backend_option_overrides(&backend)
            .await
            .expect("a valid base_url");
        assert!(SuperTTSDaemon::base_url_egress_hosts(&backend, &overrides).is_empty());

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
            SuperTTSDaemon::base_url_egress_hosts(&backend, &overrides),
            vec!["localhost:8080", "localhost"]
        );

        // A backend declaring no `base_url` option contributes nothing, even
        // with the override still in config.
        let no_base = DiscoveredBackend {
            options: vec![],
            ..backend
        };
        assert!(SuperTTSDaemon::base_url_egress_hosts(&no_base, &overrides).is_empty());
    }

    /// A `base_url` pasted with surrounding whitespace must reach both paths as
    /// the same string: the component dials the header it is given, and the
    /// daemon authorizes what the value names. A value that is only whitespace
    /// is no value — it must not be injected or authorize anything.
    #[tokio::test]
    async fn whitespace_in_base_url_cannot_split_the_two_paths() {
        use crate::daemon::test_fixtures::openai_backend;
        use crate::daemon::types::test_daemon;
        use crate::tts_models::backends::DiscoveredBackend;

        let daemon = test_daemon().await;
        let source = "github.com/super-tts/openai";
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
                .find(|(k, _)| k == "x-tts-option-base_url")
                .map(|(_, v)| v.as_str());
            let egress = SuperTTSDaemon::base_url_egress_hosts(&backend, &overrides);
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
        use crate::tts_models::backends::DiscoveredBackend;

        let daemon = test_daemon().await;
        let source = "github.com/super-tts/openai";
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
            .find(|(k, _)| k == "x-tts-option-base_url")
            .map(|(_, v)| v.as_str());
        assert_eq!(injected, Some("https://gw.example.com:8443/v1"));
        assert_eq!(
            SuperTTSDaemon::base_url_egress_hosts(&backend, &overrides),
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
        use crate::tts_models::backends::DiscoveredBackend;

        let daemon = test_daemon().await;
        let source = "github.com/super-tts/openai";
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
        use crate::tts_models::backends::DiscoveredBackend;

        let daemon = test_daemon().await;
        let source = "github.com/super-tts/openai";
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
            .find(|(k, _)| k == "x-tts-option-base_url")
            .map(|(_, v)| v.as_str())
            .expect("base_url is injected from the snapshot");
        let egress = SuperTTSDaemon::base_url_egress_hosts(&backend, &overrides);
        assert_eq!(injected, "http://10.0.0.5:8080");
        assert_eq!(egress, vec!["10.0.0.5:8080", "10.0.0.5"]);
    }
}
