// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::backends;
use log::info;
use std::path::PathBuf;

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
    pub async fn pick_startup_model(&self) -> Option<(String, String)> {
        let (pref_model, pref_source, allow_online) = {
            let c = self.config.read().await;
            (
                c.transcription.preferred_model.clone(),
                c.transcription.preferred_source.clone(),
                c.online.allow_online_models,
            )
        };
        if pref_model.is_empty() {
            return None;
        }
        // A config predating `preferred_source` names a model but not the
        // backend serving it. Resolve it the same way the wire path does — from
        // the selected backend — rather than scanning for the first backend
        // that serves the name: two backends may serve the same name, and the
        // scan order is `read_dir` order, so the daemon could come up on a
        // different engine than the one the user chose. With no selection
        // recorded either, stay idle and let the user pick.
        let pref_source = if pref_source.is_empty() {
            let Some(resolved) = self.active_backend_source().await else {
                info!(
                    "Startup model {pref_model} names no source and no backend is selected; \
                     staying idle"
                );
                return None;
            };
            resolved
        } else {
            pref_source
        };
        let backends = self.backends.read().await;
        let (_, def) = backends::find_model(&backends, &pref_model, &pref_source)?;
        // Online models need the online gate on to be usable; local models are
        // always usable (the required secret is enforced at load).
        let usable = !def.is_online() || allow_online;
        usable.then(|| (def.name.clone(), def.source.clone()))
    }

    /// First discovered local (non-online) model, if any. Used as the safe
    /// fallback when online models are turned off.
    pub async fn first_local_model(&self) -> Option<(String, String)> {
        let backends = self.backends.read().await;
        for backend in backends.iter() {
            for def in &backend.models {
                if !def.is_online() {
                    return Some((def.name.clone(), def.source.clone()));
                }
            }
        }
        None
    }
}
