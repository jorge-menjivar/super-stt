// SPDX-License-Identifier: GPL-3.0-only
use crate::tts::daemon::types::SuperTTSDaemon;
use crate::tts::tts_models::backends;
use log::info;
use std::path::PathBuf;

impl SuperTTSDaemon {
    /// The directory backends install into: the configured one, else the
    /// default.
    pub async fn backends_dir(&self) -> PathBuf {
        let configured = self.config.read().await.backends_dir.clone();
        configured.map_or_else(backends::default_backends_dir, PathBuf::from)
    }

    /// Choose the model to load at startup: the configured preference, but only
    /// if it is installed. Returns `None` (daemon stays idle)
    /// when there is no preference or it can't be loaded — the daemon never
    /// auto-picks an arbitrary model, since loading one can pull gigabytes.
    pub async fn pick_startup_model(&self) -> Option<(String, String)> {
        self.selected_model()
            .await
            .map_err(|why| {
                if let Some(why) = why {
                    info!("{why}");
                }
            })
            .ok()
    }

    /// [`pick_startup_model`](Self::pick_startup_model) without its log
    /// line, for checks that run on every request. The error is why there is
    /// no model to load, when there is one worth logging.
    pub(crate) async fn selected_model(&self) -> Result<(String, String), Option<String>> {
        let (pref_model, pref_source) = {
            let c = self.config.read().await;
            (
                c.synthesis.preferred_model.clone(),
                c.synthesis.preferred_source.clone(),
            )
        };
        if pref_model.is_empty() {
            return Err(None);
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
                return Err(Some(format!(
                    "Startup model {pref_model} names no source and no backend is selected; \
                     staying idle"
                )));
            };
            resolved
        } else {
            pref_source
        };
        let backends = self.backends.read().await;
        let (_, def) = backends::find_model(&backends, &pref_model, &pref_source).ok_or(None)?;
        // A required secret is enforced at load.
        Ok((def.name.clone(), def.source.clone()))
    }
}
