// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use log::{info, warn};
use super_stt_registry_types::manifest::{ModelRole, Opt};
use super_stt_shared::models::backends::{BackendInfo, BackendModel, BackendOption, BackendSecret};
use super_stt_shared::models::protocol::{DaemonResponse, ErrorCode};

/// Fold an optional reload-failure warning into a success message, so a failed
/// post-write model reload is surfaced to the caller instead of swallowed.
fn with_reload_warning(base: String, reload_warning: Option<String>) -> String {
    match reload_warning {
        Some(w) => format!("{base} (but reloading the running model failed: {w})"),
        None => base,
    }
}

/// The same, for a write that reconfigures rather than reloads.
///
/// The value is stored either way; what failed is getting it to a model already
/// running. The user has to be told, because the settings UI will show the new
/// value while the backend goes on using the old one.
pub(in crate::daemon) fn with_apply_warning(base: String, warning: Option<String>) -> String {
    match warning {
        Some(w) => format!("{base} (but the running backend kept the old value: {w})"),
        None => base,
    }
}

impl SuperSTTDaemon {
    /// Handle list backends command — the installed-backend catalog with each
    /// backend's models, declared secrets, and options (with effective values).
    /// Drives the settings UI; see `docs/protocol/endpoints/v1/backend/list.md`.
    pub async fn handle_list_backends(&self) -> DaemonResponse {
        let catalog = self.backend_catalog().await;
        info!("Backends catalog requested: {} backend(s)", catalog.len());
        let backends_json = serde_json::to_value(&catalog).unwrap_or_default();
        DaemonResponse::success()
            .with_backends(backends_json)
            .with_message("Backends listed successfully".to_string())
    }

    /// `GET /pipeline/{stage}/backend/list` — the installed backends that can
    /// fill this stage: those serving at least one model carrying its role.
    ///
    /// The same catalog `GET /backend/list` returns, narrowed. Narrowed *here*
    /// rather than by each client, because the daemon already decides this when
    /// it accepts or refuses `POST /pipeline/{stage}` — and a client filtering
    /// on its own can offer a backend the daemon then refuses, which is a
    /// picker that hands the user an error.
    pub async fn handle_list_stage_backends(&self, post_processor: bool) -> DaemonResponse {
        let role = ModelRole::PostProcessor.to_string();
        let catalog: Vec<BackendInfo> = self
            .backend_catalog()
            .await
            .into_iter()
            .filter_map(|mut b| {
                // Each backend's models are narrowed to this stage's role too,
                // not just the list of backends. The whole answer is about one
                // position, so a caller can render a row straight from it — and
                // a backend serving both roles must not show stage 1 the
                // post-processor it also ships.
                b.models.retain(|m| (m.role == role) == post_processor);
                (!b.models.is_empty()).then_some(b)
            })
            .collect();
        let stage = if post_processor { 2 } else { 1 };
        info!(
            "Stage {stage} backends requested: {} backend(s)",
            catalog.len()
        );
        let backends_json = serde_json::to_value(&catalog).unwrap_or_default();
        DaemonResponse::success()
            .with_backends(backends_json)
            .with_message(format!("Backends available to stage {stage} listed"))
    }

    /// The installed-backend catalog both list endpoints answer from, so a
    /// stage's view of a backend and the whole catalog's cannot differ in
    /// anything but which backends are in it.
    async fn backend_catalog(&self) -> Vec<BackendInfo> {
        let config = self.config.read().await;
        let backends = self.backends.read().await;

        let catalog: Vec<BackendInfo> = backends
            .iter()
            .map(|b| {
                let models = b
                    .models
                    .iter()
                    .map(|m| BackendModel {
                        name: m.name.clone(),
                        // Compatibility shim; see `BackendModel::provider`.
                        provider: String::new(),
                        supported_devices: m
                            .supported_devices
                            .iter()
                            .map(ToString::to_string)
                            .collect(),
                        estimated_vram_bytes: m.estimated_vram_bytes,
                        multilingual: m.is_multilingual,
                        supported_languages: m.supported_languages.clone(),
                        primary_language: m.primary_language.clone(),
                        realtime: m.realtime,
                        role: m.role.to_string(),
                    })
                    .collect();
                let secrets = b
                    .secrets
                    .iter()
                    .map(|s| BackendSecret {
                        name: s.name.clone(),
                        label: s.label.clone(),
                        description: s.description.clone(),
                        required: s.required,
                    })
                    .collect();
                let options = b
                    .options
                    .iter()
                    .map(|o| {
                        let default = o.default.as_ref().map(ToString::to_string);
                        let value = config
                            .backend_option(&b.source, &o.name)
                            .map(str::to_string)
                            .or_else(|| default.clone());
                        BackendOption {
                            name: o.name.clone(),
                            label: o.label.clone(),
                            description: o.description.clone(),
                            r#type: o.r#type.map(|t| t.as_str().to_string()),
                            default,
                            choices: o.choices.iter().map(ToString::to_string).collect(),
                            required: o.required,
                            value,
                        }
                    })
                    .collect();
                BackendInfo {
                    source: b.source.clone(),
                    name: b.name.clone(),
                    description: b.description.clone(),
                    // Re-read rather than reported from the scan: a client
                    // showing this beside an update badge would otherwise name
                    // the version the daemon started with while the badge was
                    // judged against the one on disk. Falls back to the scan's
                    // value if the manifest cannot be read now, since the last
                    // known version beats none for a backend in that state.
                    version: crate::stt_models::backends::installed_version(&b.dir)
                        .unwrap_or_else(|| b.version.clone()),
                    kind: b.kind.clone(),
                    // `"wasm"` is what `installed.json` records for a wasm-kind
                    // backend's asset — correct for that record's own purpose,
                    // but it names a transport, not an accelerator, so it is
                    // filtered before publication (see `BackendInfo::installed_accel`).
                    installed_accel: crate::registry::installed::read(&b.dir)
                        .map(|r| r.selected.accel)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|a| a != "wasm")
                        .collect(),
                    // The manifest's declared egress, and only that. A user-set
                    // `base_url` authorizes an endpoint beyond it, but it is the
                    // user's own value and does not belong in a field clients
                    // read as "what this backend declared": the settings UI
                    // reports it from the `base_url` option instead.
                    allowed_hosts: b.allowed_hosts.clone(),
                    models,
                    secrets,
                    options,
                }
            })
            .collect();
        catalog
    }

    /// Reload every stage running a model from `source`, so a just-changed
    /// secret takes effect immediately. Returns a warning message if a reload
    /// was attempted and failed (so the caller can surface it), or `None`
    /// otherwise.
    ///
    /// Both stages, because both run backend models: a post-processor holds the
    /// API key of its backend exactly as a transcription model does, and
    /// reloading only stage 1 left it running with the value the user had just
    /// replaced, with nothing saying so.
    ///
    /// Options do not come through here — see
    /// [`reconfigure_if_source_active`](Self::reconfigure_if_source_active).
    /// Secrets still do, deliberately: the same argument says they need not,
    /// since a secret is a request header too, but a backend plausibly does
    /// something with a credential when it loads, and a key is not changed
    /// often enough for the conservative path to cost anything.
    async fn reload_if_source_active(&self, source: &str) -> Option<String> {
        let transcription = self
            .model
            .read()
            .await
            .as_ref()
            .map(|l| l.definition.source.clone());
        let mut warnings = Vec::new();
        if transcription.as_deref() == Some(source) {
            let resp = self.handle_reload_active_model().await;
            if resp.status != "success" {
                warnings.push(resp.message.unwrap_or_else(|| "unknown error".to_string()));
            }
        }
        if self.post_processor_source().await.as_deref() == Some(source) {
            let resp = self.handle_reload_post_processor().await;
            if resp.status != "success" {
                warnings.push(resp.message.unwrap_or_else(|| "unknown error".to_string()));
            }
        }
        (!warnings.is_empty()).then(|| warnings.join("; "))
    }

    /// Hand every stage running a model from `source` a freshly resolved
    /// [`BackendContext`](crate::stt_models::transcribe::BackendContext), so a
    /// just-changed option takes effect on the next request.
    ///
    /// Both stages, for the same reason the reload path does both: a
    /// post-processor reads its backend's options off the request exactly as a
    /// transcription model does, and `super-stt-tidy`'s six toggles are the
    /// options most likely to be changed at all.
    ///
    /// Re-resolved rather than patched in place, because the injected headers
    /// and the egress list have to come from one snapshot of the options and
    /// only [`backend_context`](Self::backend_context) produces that pair.
    /// Secrets are re-read on the way through; a keyring round-trip per
    /// settings write costs nothing and keeps this on the code path a load
    /// already uses, so the two cannot resolve a value differently.
    ///
    /// Returns a warning if the new context could not be resolved. The stages
    /// keep running on the old one, and the caller has to surface that: the
    /// value is stored, so nothing else would tell the user it is not in use.
    pub(in crate::daemon) async fn reconfigure_if_source_active(
        &self,
        source: &str,
    ) -> Option<String> {
        #[cfg(any(feature = "wasm-backends", feature = "subprocess-backends"))]
        {
            let transcription = self
                .model
                .read()
                .await
                .as_ref()
                .map(|l| l.definition.source.clone());
            let processor = self.post_processor_source().await;
            if transcription.as_deref() != Some(source) && processor.as_deref() != Some(source) {
                return None;
            }
            let backend = {
                let backends = self.backends.read().await;
                backends.iter().find(|b| b.source == source).cloned()
            };
            // Loaded from a backend that is not in the catalog: there is no
            // manifest to resolve a context against, so the value is stored and
            // undelivered — which is the caller's to report, exactly like a
            // context that fails to resolve below.
            let Some(backend) = backend else {
                return Some(format!("{source} is not installed"));
            };
            // Resolved without either slot held. It reads config and the
            // keyring, and the transcribe path holds those slots for a whole
            // inference.
            let context = match self.backend_context(&backend).await {
                Ok(context) => context,
                Err(e) => return Some(e.to_string()),
            };
            // Re-checked under the guard, because the slots were unlocked while
            // the context resolved: pushing settings into whatever is loaded
            // *now* would hand one backend another backend's configuration.
            if let Some(loaded) = self.model.read().await.as_ref()
                && loaded.definition.source == source
            {
                loaded.instance.reconfigure(context.clone());
            }
            if let Some(loaded) = self.post_processor.read().await.as_ref()
                && loaded.definition.source == source
            {
                loaded.instance.reconfigure(context);
            }
            None
        }
        #[cfg(not(any(feature = "wasm-backends", feature = "subprocess-backends")))]
        {
            let _ = source;
            None
        }
    }

    /// Hand *every* running stage a freshly resolved
    /// [`BackendContext`](crate::stt_models::transcribe::BackendContext),
    /// whatever backend each one runs.
    ///
    /// The sibling of [`reconfigure_if_source_active`](Self::reconfigure_if_source_active),
    /// for a change that is not about one backend. A dictation context is
    /// global: switching it moves both stages at once, and the two stages may
    /// run different backends — which is the case the `source`-keyed helper
    /// gets wrong, since it resolves one context and hands the same one to both
    /// slots. It only ever does the right thing there because it returns early
    /// unless the source it was given is the one loaded.
    ///
    /// Resolved once per *distinct* source, so the ordinary case of both stages
    /// on one backend costs one resolution rather than two, and delegated per
    /// source so the re-check-under-the-guard discipline is written once.
    ///
    /// Returns a warning naming what could not be re-resolved. The stages keep
    /// running on what they had, and the caller has to surface it: the change
    /// is stored, so nothing else would tell the user it is not in use.
    pub(in crate::daemon) async fn reconfigure_active_stages(&self) -> Option<String> {
        let mut sources: Vec<String> = Vec::new();
        if let Some(source) = self
            .model
            .read()
            .await
            .as_ref()
            .map(|l| l.definition.source.clone())
        {
            sources.push(source);
        }
        if let Some(source) = self.post_processor_source().await
            && !sources.contains(&source)
        {
            sources.push(source);
        }

        let mut warnings = Vec::new();
        for source in sources {
            if let Some(warning) = self.reconfigure_if_source_active(&source).await {
                warnings.push(warning);
            }
        }
        (!warnings.is_empty()).then(|| warnings.join("; "))
    }

    /// The manifest declaration of one option of one installed backend, or
    /// `None` when either is unknown here.
    ///
    /// Cloned rather than returned behind the guard: the caller goes on to take
    /// a write lock on the config, and holding a read guard on the catalog
    /// across that is how two settings writes deadlock each other.
    async fn declared_option(&self, source: &str, name: &str) -> Option<Opt> {
        let backends = self.backends.read().await;
        backends
            .iter()
            .find(|b| b.source == source)?
            .options
            .iter()
            .find(|o| o.name == name)
            .cloned()
    }

    /// Handle set backend option command — store/clear a plaintext option
    /// override in config, and hand it to the stages already running.
    ///
    /// Takes effect on the backend's next request, not its next model load.
    /// Options are injected as `x-stt-option-*` headers on every `/v1` call and
    /// read there, so the value reaching a running instance is a matter of
    /// swapping what it injects; it used to reload the model instead — both
    /// stages of it — which unmapped and remapped gigabytes of weights to
    /// change a flag.
    pub async fn handle_set_backend_option(
        &self,
        source: String,
        name: String,
        value: String,
    ) -> DaemonResponse {
        // A write that changes nothing does nothing. Worth its own check
        // because the settings UI cannot help sending them: a toggle re-set to
        // where it already was is a write, as is every re-pick of the value
        // already showing.
        {
            let config = self.config.read().await;
            let unchanged = match config.backend_option(&source, &name) {
                Some(stored) => stored == value,
                None => value.is_empty(),
            };
            if unchanged {
                return DaemonResponse::success().with_message(format!("Option {name} unchanged"));
            }
        }
        // Refuse a value the transports cannot carry, before it is stored. An
        // option value is injected as an `x-stt-option-*` request header, and a
        // header holds neither a control character nor an unbounded number of
        // bytes — so storing one reports success and then breaks every request
        // the backend makes, naming nothing the user set.
        //
        // Here rather than in the HTTP handler because this is the one place
        // every caller reaches, and the one holding the manifest the declared
        // type comes from. An option no installed backend declares is left
        // alone: nothing injects it (see `backend_headers`), so there is no
        // delivery to protect, and the HTTP path has already refused that write
        // with `unknown_option`.
        if !value.is_empty()
            && let Some(opt) = self.declared_option(&source, &name).await
            && let Err(e) = opt.permits_shape(&value)
        {
            return DaemonResponse::error_with_code(ErrorCode::InvalidValue, &e);
        }
        {
            let mut config = self.config.write().await;
            config.update_backend_option(source.clone(), name.clone(), value.clone());
        }
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after backend option update: {e}");
        }

        let warning = self.reconfigure_if_source_active(&source).await;

        let base = if value.is_empty() {
            info!("Cleared backend option {name} for {source}");
            format!("Option {name} cleared")
        } else {
            info!("Set backend option {name} for {source}");
            format!("Option {name} updated")
        };
        DaemonResponse::success().with_message(with_apply_warning(base, warning))
    }

    /// Store (or replace) a backend secret and reload the active model if needed.
    pub async fn handle_set_backend_secret(
        &self,
        source: String,
        name: String,
        value: String,
    ) -> DaemonResponse {
        if let Err(e) =
            crate::keyring::set_backend_secret_async(source.clone(), name.clone(), value).await
        {
            return DaemonResponse::error(&format!("keyring_unavailable: {e}"));
        }
        let reload_warning = self.reload_if_source_active(&source).await;
        info!("Set backend secret {name} for {source}");
        DaemonResponse::success().with_message(with_reload_warning(
            format!("Secret {name} stored"),
            reload_warning,
        ))
    }

    /// Clear a backend secret (reset to unset) and reload the active model if needed.
    pub async fn handle_clear_backend_secret(
        &self,
        source: String,
        name: String,
    ) -> DaemonResponse {
        if let Err(e) =
            crate::keyring::delete_backend_secret_async(source.clone(), name.clone()).await
        {
            return DaemonResponse::error(&format!("keyring_unavailable: {e}"));
        }
        let reload_warning = self.reload_if_source_active(&source).await;
        info!("Cleared backend secret {name} for {source}");
        DaemonResponse::success().with_message(with_reload_warning(
            format!("Secret {name} cleared"),
            reload_warning,
        ))
    }
}
