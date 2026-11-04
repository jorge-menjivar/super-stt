// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperTTSDaemon;
use log::{info, warn};
use super_tts_registry_types::manifest::Opt;
use super_tts_shared::models::backends::{BackendInfo, BackendModel, BackendOption, BackendSecret};
use super_tts_shared::models::protocol::{DaemonResponse, ErrorCode};

/// Fold an optional reload-failure warning into a success message, so a failed
/// post-write model reload is surfaced to the caller instead of swallowed.
fn with_reload_warning(base: String, reload_warning: Option<String>) -> String {
    match reload_warning {
        Some(w) => format!("{base} (but reloading the active model failed: {w})"),
        None => base,
    }
}

/// The same, for a write that reconfigures rather than reloads.
///
/// The value is stored either way; what failed is getting it to the model
/// already running. The user has to be told, because the settings UI will show
/// the new value while the backend goes on using the old one.
fn with_apply_warning(base: String, warning: Option<String>) -> String {
    match warning {
        Some(w) => format!("{base} (but the running backend kept the old value: {w})"),
        None => base,
    }
}

impl SuperTTSDaemon {
    /// Handle list backends command — the installed-backend catalog with each
    /// backend's models, declared secrets, and options (with effective values).
    /// Drives the settings UI; see `docs/protocol/endpoints/v1/backends.md`.
    pub async fn handle_list_backends(&self) -> DaemonResponse {
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
                            min: o.min,
                            max: o.max,
                            step: o.step,
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
                    version: crate::tts_models::backends::installed_version(&b.dir)
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

        info!("Backends catalog requested: {} backend(s)", catalog.len());
        let backends_json = serde_json::to_value(&catalog).unwrap_or_default();
        DaemonResponse::success()
            .with_backends(backends_json)
            .with_message("Backends listed successfully".to_string())
    }

    /// Reload the active model iff it is served by `source`, so a just-changed
    /// secret takes effect immediately. Returns a warning message if a reload
    /// was attempted and failed (so the caller can surface it), or `None`
    /// otherwise.
    ///
    /// Options do not come through here — see
    /// [`reconfigure_if_source_active`](Self::reconfigure_if_source_active).
    /// Secrets still do, deliberately: the same argument says they need not,
    /// since a secret is a request header too, but a backend plausibly does
    /// something with a credential when it loads, and a key is not changed
    /// often enough for the conservative path to cost anything.
    async fn reload_if_source_active(&self, source: &str) -> Option<String> {
        let active_source = self
            .model
            .read()
            .await
            .as_ref()
            .map(|l| l.definition.source.clone());
        if active_source.as_deref() == Some(source) {
            let resp = self.handle_reload_active_model().await;
            if resp.status != "success" {
                return Some(resp.message.unwrap_or_else(|| "unknown error".to_string()));
            }
        }
        None
    }

    /// Hand the active model a freshly resolved
    /// [`BackendContext`](crate::tts_models::synthesize::BackendContext) iff it
    /// is served by `source`, so a just-changed option takes effect on the next
    /// request.
    ///
    /// Re-resolved rather than patched in place, because the injected headers
    /// and the egress list have to come from one snapshot of the options and
    /// only [`backend_context`](Self::backend_context) produces that pair.
    /// Secrets are re-read on the way through; a keyring round-trip per
    /// settings write costs nothing and keeps this on the code path a load
    /// already uses, so the two cannot resolve a value differently.
    ///
    /// Returns a warning if the new context could not be resolved. The model
    /// keeps running on the old one, and the caller has to surface that: the
    /// value is stored, so nothing else would tell the user the backend is not
    /// using it.
    async fn reconfigure_if_source_active(&self, source: &str) -> Option<String> {
        #[cfg(any(feature = "wasm-backends", feature = "subprocess-backends"))]
        {
            let active = self
                .model
                .read()
                .await
                .as_ref()
                .map(|l| l.definition.source.clone());
            if active.as_deref() != Some(source) {
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
            // Resolved without the model slot held. It reads config and the
            // keyring, and the speak path holds that same slot for a whole
            // synthesis.
            let context = match self.backend_context(&backend).await {
                Ok(context) => context,
                Err(e) => return Some(e.to_string()),
            };
            let guard = self.model.read().await;
            match guard.as_ref() {
                // Re-checked, because the slot was unlocked while the context
                // resolved: pushing settings into whatever is loaded *now*
                // would hand one backend another backend's configuration.
                Some(loaded) if loaded.definition.source == source => {
                    loaded.instance.reconfigure(context);
                    None
                }
                _ => None,
            }
        }
        #[cfg(not(any(feature = "wasm-backends", feature = "subprocess-backends")))]
        {
            let _ = source;
            None
        }
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
    /// override in config, and hand it to the running backend.
    ///
    /// Takes effect on the backend's next request, not its next model load.
    /// Options are injected as `x-tts-option-*` headers on every `/v1` call and
    /// read there, so the value reaching a running instance is a matter of
    /// swapping what it injects; it used to reload the model instead, which
    /// unmapped and remapped gigabytes of weights to change a number.
    pub async fn handle_set_backend_option(
        &self,
        source: String,
        name: String,
        value: String,
    ) -> DaemonResponse {
        // A write that changes nothing does nothing. Worth its own check
        // because the settings UI cannot help sending them: a slider commits on
        // release, so grabbing one and putting it back where it was is a write,
        // as is every re-pick of the value already showing.
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
        // option value is injected as an `x-tts-option-*` request header, and a
        // header holds neither a control character nor an unbounded number of
        // bytes — so storing one reports success and then breaks every request
        // the backend makes, naming nothing the user set.
        //
        // Here rather than in the HTTP handler because this is the one place
        // every caller reaches, and the one holding the manifest the declared
        // type comes from. An option no installed backend declares is left
        // alone: nothing injects it, so there is no delivery to protect, and
        // the HTTP path has already refused that write with `unknown_option`.
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
