// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use log::info;
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

        let catalog: Vec<serde_json::Value> = backends
            .iter()
            .map(|b| {
                let models: Vec<serde_json::Value> = b
                    .models
                    .iter()
                    .map(|m| {
                        serde_json::json!({
                            "name": m.name,
                            "provider": m.provider.to_string(),
                            "multilingual": m.is_multilingual,
                            "primary_language": m.primary_language,
                            "supported_languages": m.supported_languages,
                            "supported_devices": m.supported_devices,
                            "estimated_vram_bytes": m.estimated_vram_bytes,
                            "realtime": m.realtime,
                        })
                    })
                    .collect();
                let secrets: Vec<serde_json::Value> = b
                    .secrets
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "name": s.name,
                            "label": s.label,
                            "description": s.description,
                            "required": s.required,
                        })
                    })
                    .collect();
                let options: Vec<serde_json::Value> = b
                    .options
                    .iter()
                    .map(|o| {
                        let default = o.default.as_ref().map(ToString::to_string);
                        let value = config
                            .backend_option(&b.source, &o.name)
                            .map(str::to_string)
                            .or_else(|| default.clone());
                        serde_json::json!({
                            "name": o.name,
                            "label": o.label,
                            "description": o.description,
                            "type": o.r#type,
                            "default": default,
                            "required": o.required,
                            "value": value,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "source": b.source,
                    "name": b.name,
                    "kind": b.kind,
                    "allowed_hosts": b.allowed_hosts,
                    "models": models,
                    "secrets": secrets,
                    "options": options,
                })
            })
            .collect();

        info!("Backends catalog requested: {} backend(s)", catalog.len());
        DaemonResponse::success()
            .with_backends(serde_json::json!(catalog))
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
        if let Err(e) = crate::keyring::set_backend_secret(&source, &name, &value) {
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
        if let Err(e) = crate::keyring::delete_backend_secret(&source, &name) {
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
