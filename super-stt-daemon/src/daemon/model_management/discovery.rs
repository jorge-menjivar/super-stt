// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::backends;
use log::info;
use std::path::PathBuf;
use super_stt_shared::models::provider::Provider;

impl SuperSTTDaemon {
    /// Re-scan the backends directory and refresh the in-memory registry.
    pub async fn refresh_backends(&self) {
        let configured = {
            let c = self.config.read().await;
            c.transcription.backends_dir.clone()
        };
        let dir = configured.map_or_else(backends::default_backends_dir, PathBuf::from);
        let discovered = backends::discover(&dir);
        info!(
            "Backend registry: {} backend(s) from {}",
            discovered.len(),
            dir.display()
        );
        *self.backends.write().await = discovered;
    }

    /// Choose the model to load at startup: the configured preference, but only
    /// if it is installed and usable. Online models are "usable" only when the
    /// online toggle is on and a key exists. Returns `None` (daemon stays idle)
    /// when there is no preference or it can't be loaded — the daemon never
    /// auto-picks an arbitrary model, since loading one can pull gigabytes.
    pub async fn pick_startup_model(&self) -> Option<(String, Provider, String)> {
        let (pref_model, pref_provider, pref_source, allow_online) = {
            let c = self.config.read().await;
            (
                c.transcription.preferred_model.clone(),
                c.transcription.preferred_provider,
                c.transcription.preferred_source.clone(),
                c.online.allow_online_models,
            )
        };
        if pref_model.is_empty() {
            return None;
        }
        let backends = self.backends.read().await;
        let (_, def) = backends::find_model(&backends, &pref_model, pref_provider, &pref_source)?;
        Self::provider_usable(def.provider, allow_online)
            .then(|| (def.name.clone(), def.provider, def.source.clone()))
    }

    /// First discovered local (non-online) model, if any. Used as the safe
    /// fallback when online models are turned off.
    pub async fn first_local_model(&self) -> Option<(String, Provider, String)> {
        let backends = self.backends.read().await;
        for backend in backends.iter() {
            for def in &backend.models {
                if !matches!(def.provider, Provider::Online(_)) {
                    return Some((def.name.clone(), def.provider, def.source.clone()));
                }
            }
        }
        None
    }

    fn provider_usable(provider: Provider, allow_online: bool) -> bool {
        match provider {
            // Online backends only need the online gate to be usable; whether
            // the required secret is set is enforced at load (`backend_headers`).
            Provider::Online(_) => allow_online,
            _ => true,
        }
    }
}
