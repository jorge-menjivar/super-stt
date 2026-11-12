// SPDX-License-Identifier: GPL-3.0-only
//! What the voices page says about the model loaded on demand. See
//! [`crate::config::IdleUnloadConfig`]. The load on demand and the idle
//! unload themselves are the engine's
//! ([`StageExt`](super_engine_daemon::stage::StageExt)), the same for every
//! stage.

use crate::tts::daemon::types::SuperTTSDaemon;

impl SuperTTSDaemon {
    /// The model the next request loads when it finds none in memory: the
    /// selected one, when models are unloaded on idle. Quiet: the voices page
    /// reads it on every listing.
    pub(crate) async fn model_loaded_on_demand(
        &self,
    ) -> Option<crate::tts::tts_models::ModelDefinition> {
        self.config.read().await.idle_unload.after()?;
        let (name, source) = self.selected_model().await.ok()?;
        self.resolve_definition(&name, &source).await
    }
}
