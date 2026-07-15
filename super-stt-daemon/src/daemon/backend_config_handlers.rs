// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use log::{info, warn};
use super_stt_shared::models::backends::{BackendInfo, BackendModel, BackendOption, BackendSecret};
use super_stt_shared::models::protocol::DaemonResponse;

/// Fold an optional reload-failure warning into a success message, so a failed
/// post-write model reload is surfaced to the caller instead of swallowed.
fn with_reload_warning(base: String, reload_warning: Option<String>) -> String {
    match reload_warning {
        Some(w) => format!("{base} (but reloading the active model failed: {w})"),
        None => base,
    }
}

impl SuperSTTDaemon {
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
                        provider: m.provider.to_string(),
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
                            required: o.required,
                            value,
                        }
                    })
                    .collect();
                BackendInfo {
                    source: b.source.clone(),
                    name: b.name.clone(),
                    kind: b.kind.clone(),
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
    /// option/secret takes effect immediately. Shared by option and secret
    /// writes. Returns a warning message if a reload was attempted and failed
    /// (so the caller can surface it), or `None` otherwise.
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
