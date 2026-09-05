// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use log::{info, warn};
use super_stt_registry_types::manifest::ModelRole;
use super_stt_shared::models::backends::{BackendInfo, BackendModel, BackendOption, BackendSecret};
use super_stt_shared::models::protocol::DaemonResponse;

/// Fold an optional reload-failure warning into a success message, so a failed
/// post-write model reload is surfaced to the caller instead of swallowed.
fn with_reload_warning(base: String, reload_warning: Option<String>) -> String {
    match reload_warning {
        Some(w) => format!("{base} (but reloading the running model failed: {w})"),
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
    /// option/secret takes effect immediately. Shared by option and secret
    /// writes. Returns a warning message if a reload was attempted and failed
    /// (so the caller can surface it), or `None` otherwise.
    ///
    /// Both stages, because both run backend models: a post-processor holds the
    /// API key and options of its backend exactly as a transcription model
    /// does, and reloading only stage 1 left it running with the value the user
    /// had just replaced, with nothing saying so.
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

    /// Handle set backend option command — store/clear a plaintext option
    /// override in config. Takes effect on the backend's next model load.
    pub async fn handle_set_backend_option(
        &self,
        source: String,
        name: String,
        value: String,
    ) -> DaemonResponse {
        {
            let mut config = self.config.write().await;
            config.update_backend_option(source.clone(), name.clone(), value.clone());
        }
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after backend option update: {e}");
        }

        let reload_warning = self.reload_if_source_active(&source).await;

        let base = if value.is_empty() {
            info!("Cleared backend option {name} for {source}");
            format!("Option {name} cleared")
        } else {
            info!("Set backend option {name} for {source}");
            format!("Option {name} updated")
        };
        DaemonResponse::success().with_message(with_reload_warning(base, reload_warning))
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
