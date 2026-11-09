// SPDX-License-Identifier: GPL-3.0-only
use crate::tts::daemon::types::SuperTTSDaemon;
use axum::extract::FromRef;
use std::sync::Arc;
use super_engine_daemon::auth::Auth;

/// What text to speech's handlers are handed: the feature, and the daemon's one
/// [`Auth`], which the guards check requests against.
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) daemon: Arc<SuperTTSDaemon>,
    pub(crate) auth: Auth,
}

impl AppState {
    pub(crate) fn new(daemon: Arc<SuperTTSDaemon>, auth: Auth) -> Self {
        Self { daemon, auth }
    }
}

/// Lets the guards extract the [`Auth`] alone.
impl FromRef<AppState> for Auth {
    fn from_ref(state: &AppState) -> Self {
        state.auth.clone()
    }
}
