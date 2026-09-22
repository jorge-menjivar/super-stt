// SPDX-License-Identifier: GPL-3.0-only
//! Dictation contexts: storing them, and choosing which one is in force.
//!
//! Contract: `docs/protocol/endpoints/v1/context.md`. The stored shape is
//! [`ContextsConfig`](crate::config::ContextsConfig) and the resolution rule —
//! which context a given backend is actually sent — is
//! [`DaemonConfig::resolve_context`](crate::config::DaemonConfig::resolve_context).
//!
//! Every write here goes through the same three steps: refuse what cannot be
//! stored, mutate and persist under [`set_config_field`], then tell subscribers
//! the set moved. The third step is what a second settings window depends on:
//! contexts are global, so one app renaming one is something every other app
//! showing the list needs to hear about.

use crate::daemon::types::SuperSTTDaemon;
use log::info;
use super_stt_shared::models::contexts::DictationContext;
use super_stt_shared::models::protocol::{DaemonResponse, ErrorCode};

impl SuperSTTDaemon {
    /// The SSE `settings_changed` topic every context write announces.
    ///
    /// One name for the whole family rather than one per verb: a client's
    /// answer to any of them is the same — re-read the list — and a topic per
    /// verb would only give it more ways to spell the same refetch.
    const CONTEXTS_TOPIC: &str = "contexts";

    /// Store a context, replacing one with the same id or adding it to the end.
    ///
    /// The vocabulary is normalized on the way in: blank terms dropped, each
    /// one trimmed. The settings UI edits terms as one input per row and keeps
    /// an empty row for the next one, so blanks arrive by design — cleaning
    /// here means neither the stored file nor any backend has to know that.
    pub async fn handle_set_context(&self, context: DictationContext) -> DaemonResponse {
        let context = DictationContext {
            vocabulary: DictationContext::clean_vocabulary(&context.vocabulary),
            ..context
        };
        // Checked here rather than at the HTTP layer because this is the one
        // place every caller reaches. A context that is too long to deliver is
        // refused rather than stored: the headers it becomes cannot carry it,
        // and storing it would report success and then quietly send nothing.
        if let Err(e) = context.check() {
            return DaemonResponse::error_with_code(ErrorCode::InvalidValue, &e);
        }
        let name = context.name.clone();
        let id = context.id.clone();
        let persist = self
            .set_config_field(move |config| config.upsert_context(context))
            .await;
        self.publish_settings_changed(Self::CONTEXTS_TOPIC);
        info!("Stored dictation context {id}");
        Self::settings_saved(
            DaemonResponse::success(),
            format!("Context {name} saved"),
            persist,
        )
    }

    /// Remove a context.
    ///
    /// Deleting the active one clears the selection, so the daemon never holds
    /// an active id with nothing behind it. A backend *pinned* to it keeps its
    /// pin — see [`DaemonConfig::remove_context`](crate::config::DaemonConfig::remove_context)
    /// — because re-creating the context should restore what the user set
    /// rather than leave them to find the pin again.
    pub async fn handle_delete_context(&self, id: String) -> DaemonResponse {
        // Read first so a delete of nothing is a 404 rather than a success
        // that did nothing. A client deleting a row it is looking at has a
        // stale list, and saying so is how it learns to refetch.
        if self.config.read().await.context(&id).is_none() {
            return DaemonResponse::error_with_code(
                ErrorCode::NotFound,
                &format!("No context with id {id}"),
            );
        }
        let removing = id.clone();
        let persist = self
            .set_config_field(move |config| {
                config.remove_context(&removing);
            })
            .await;
        self.publish_settings_changed(Self::CONTEXTS_TOPIC);
        info!("Deleted dictation context {id}");
        Self::settings_saved(
            DaemonResponse::success(),
            format!("Context {id} deleted"),
            persist,
        )
    }

    /// Choose the context in force by default, or clear the selection.
    pub async fn handle_set_active_context(&self, id: Option<String>) -> DaemonResponse {
        if let Some(wanted) = id.as_deref()
            && self.config.read().await.context(wanted).is_none()
        {
            return DaemonResponse::error_with_code(
                ErrorCode::NotFound,
                &format!("No context with id {wanted}"),
            );
        }
        let chosen = id.clone();
        let persist = self
            .set_config_field(move |config| config.set_active_context(chosen))
            .await;
        self.publish_settings_changed(Self::CONTEXTS_TOPIC);
        let message = if let Some(id) = id {
            info!("Active dictation context is now {id}");
            format!("Context {id} is now active")
        } else {
            info!("Cleared the active dictation context");
            "No context is active".to_string()
        };
        Self::settings_saved(DaemonResponse::success(), message, persist)
    }

    /// Point one backend at a context of its own, or put it back on the active
    /// one.
    ///
    /// `id` carries three states here, not two: `None` clears the override,
    /// `Some(id)` pins, and `Some("")` means send this backend no context at
    /// all. The empty string is not a valid context id, so it cannot collide
    /// with one the user made.
    ///
    /// Whether `source` names an installed backend is not checked. A pin can
    /// legitimately outlive an uninstall — reinstalling the backend should
    /// find its setting where it left it — and the HTTP path already refuses
    /// an unknown backend with `unknown_backend`, which is the answer a client
    /// typing a source by hand wants.
    pub async fn handle_set_backend_context(
        &self,
        source: String,
        id: Option<String>,
    ) -> DaemonResponse {
        if let Some(wanted) = id.as_deref()
            && !wanted.is_empty()
            && self.config.read().await.context(wanted).is_none()
        {
            return DaemonResponse::error_with_code(
                ErrorCode::NotFound,
                &format!("No context with id {wanted}"),
            );
        }
        let (pinned, chosen) = (source.clone(), id.clone());
        let persist = self
            .set_config_field(move |config| config.update_backend_context(pinned, chosen))
            .await;
        self.publish_settings_changed(Self::CONTEXTS_TOPIC);
        let message = match id.as_deref() {
            None => format!("{source} follows the active context"),
            Some("") => format!("{source} uses no context"),
            Some(id) => format!("{source} uses context {id}"),
        };
        info!("{message}");
        Self::settings_saved(DaemonResponse::success(), message, persist)
    }
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
