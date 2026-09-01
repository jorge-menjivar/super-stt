// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::backends::BackendOption;
use crate::daemon::client::{
    clear_backend_option, clear_backend_secret, list_backend_secrets, set_backend_option,
    set_backend_secret,
};
use crate::state::ErrorScope;
use crate::ui::messages::{BackendMessage, Message};
use cosmic::prelude::*;
use std::future::Future;
use super_stt_shared::daemon::http_client::{HttpError, HttpResult};

/// Build a Configure-sheet-scoped banner message for a failed secret/option save.
fn configure_backend_error(e: &HttpError) -> Message {
    Message::SettingActionFailed {
        scope: ErrorScope::ConfigureBackend,
        message: e.to_string(),
    }
}

/// Run an option write and settle the sheet either way: reload the catalog so
/// the row re-renders from what the daemon actually stored, or surface the
/// failure in the sheet's banner. Set, clear and toggle all end here.
fn option_write(
    write: impl Future<Output = HttpResult<()>> + Send + 'static,
) -> Task<cosmic::Action<Message>> {
    Task::perform(write, |result| match result {
        Ok(()) => cosmic::Action::App(Message::Backend(BackendMessage::BackendsReload)),
        Err(e) => cosmic::Action::App(configure_backend_error(&e)),
    })
}

impl AppModel {
    /// Backend catalog refresh + per-backend secret/option configuration
    /// (used by the Configure sub-view).
    pub(in crate::core::app) fn handle_backend_messages(
        &mut self,
        message: BackendMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            BackendMessage::BackendsLoaded(_)
            | BackendMessage::BackendsError(_)
            | BackendMessage::BackendsReload
            | BackendMessage::BackendSecretsConfigured { .. } => {
                self.handle_backend_catalog(message)
            }

            BackendMessage::BackendSecretInputChanged { .. }
            | BackendMessage::BackendSecretSaved { .. }
            | BackendMessage::BackendSecretStored { .. }
            | BackendMessage::BackendSecretRemoved { .. }
            | BackendMessage::BackendOptionInputChanged { .. }
            | BackendMessage::BackendOptionSaved { .. }
            | BackendMessage::BackendOptionToggled { .. }
            | BackendMessage::BackendOptionReset { .. } => self.handle_backend_config(message),
        }
    }

    /// Handle backend catalog messages: `BackendsLoaded`, `BackendsError`, `BackendsReload`,
    /// `BackendSecretsConfigured`.
    fn handle_backend_catalog(&mut self, message: BackendMessage) -> Task<cosmic::Action<Message>> {
        match message {
            BackendMessage::BackendsLoaded(backends) => {
                // Prefill option input buffers from each option's current value.
                // Secret configured-flags are now daemon-sourced: dispatch
                // list_backend_secrets per backend and fold via BackendSecretsConfigured.
                self.backend_option_inputs.clear();
                self.backend_secret_configured.clear();
                for backend in &backends {
                    for option in &backend.options {
                        self.backend_option_inputs.insert(
                            (backend.source.clone(), option.name.clone()),
                            option.value.clone().unwrap_or_default(),
                        );
                    }
                }
                // Drop uninstall errors for backends no longer present.
                self.registry
                    .uninstall_errors
                    .retain(|src, _| backends.iter().any(|b| &b.source == src));
                let mut tasks: Vec<_> = backends
                    .iter()
                    .map(|b| {
                        let source = b.source.clone();
                        Task::perform(list_backend_secrets(source.clone()), move |res| {
                            let items = res.unwrap_or_default();
                            cosmic::Action::App(Message::Backend(
                                BackendMessage::BackendSecretsConfigured {
                                    source: source.clone(),
                                    items,
                                },
                            ))
                        })
                    })
                    .collect();
                self.backends = backends;
                // Load the registry index once so installed cards can show each
                // backend's description (it lives on the registry entry, keyed
                // by source). Guarded so it fires a single time; the Browse tab's
                // own first-open trigger then short-circuits.
                if self.registry.backends.is_empty() && self.registry.last_refresh.is_none() {
                    tasks.push(crate::core::app::handlers::tasks::fetch_registry_catalog(
                        false,
                    ));
                }
                Task::batch(tasks)
            }

            BackendMessage::BackendSecretsConfigured { source, items } => {
                for (name, configured) in items {
                    self.backend_secret_configured
                        .insert((source.clone(), name), configured);
                }
                Task::none()
            }

            BackendMessage::BackendsError(err) => {
                log::warn!("Backends load error: {err}");
                Task::none()
            }

            BackendMessage::BackendsReload => crate::core::app::handlers::tasks::reload_backends(),

            _ => Task::none(),
        }
    }

    /// The `default` a `bool` option declares, read as a bool.
    ///
    /// A switch flipped back to it is reverting, not choosing, so the write
    /// clears the override instead of storing a copy of the default.
    fn option_bool_default(&self, source: &str, name: &str) -> Option<bool> {
        self.backends
            .iter()
            .find(|b| b.source == source)
            .and_then(|b| b.options.iter().find(|o| o.name == name))
            .and_then(BackendOption::bool_default)
    }

    /// Handle per-backend secret and option configuration messages.
    fn handle_backend_config(&mut self, message: BackendMessage) -> Task<cosmic::Action<Message>> {
        match message {
            BackendMessage::BackendSecretInputChanged {
                source,
                name,
                value,
            } => {
                self.backend_secret_inputs.insert((source, name), value);
                Task::none()
            }

            // Send secret to daemon; the daemon reloads-if-active itself,
            // so we only need to refresh the catalog to pick up the new
            // configured flag.
            // The input buffer is NOT cleared here — it is cleared only on
            // success via BackendSecretStored, so a transient failure leaves
            // the typed value intact for retry.
            BackendMessage::BackendSecretSaved { source, name } => {
                // Clear any stale Configure-sheet banner as the user retries.
                self.action_error = None;
                let key = (source.clone(), name.clone());
                let Some(value) = self.backend_secret_inputs.get(&key).cloned() else {
                    return Task::none();
                };
                if value.is_empty() {
                    return Task::none();
                }
                Task::perform(
                    set_backend_secret(source.clone(), name.clone(), value),
                    move |res| match res {
                        Ok(()) => cosmic::Action::App(Message::Backend(
                            BackendMessage::BackendSecretStored {
                                source: source.clone(),
                                name: name.clone(),
                            },
                        )),
                        Err(e) => cosmic::Action::App(configure_backend_error(&e)),
                    },
                )
            }

            // Daemon confirmed the secret was written — clear the input buffer
            // and refresh the catalog so the configured flag updates.
            BackendMessage::BackendSecretStored { source, name } => {
                self.backend_secret_inputs.remove(&(source, name));
                self.handle_backend_catalog(BackendMessage::BackendsReload)
            }

            // Clear secret via daemon; daemon reloads-if-active itself.
            BackendMessage::BackendSecretRemoved { source, name } => {
                self.action_error = None;
                self.backend_secret_inputs
                    .remove(&(source.clone(), name.clone()));
                Task::perform(clear_backend_secret(source, name), move |res| match res {
                    Ok(()) => cosmic::Action::App(Message::Backend(BackendMessage::BackendsReload)),
                    Err(e) => cosmic::Action::App(configure_backend_error(&e)),
                })
            }

            BackendMessage::BackendOptionInputChanged {
                source,
                name,
                value,
            } => {
                self.backend_option_inputs.insert((source, name), value);
                Task::none()
            }

            // Options go to the daemon config. If the input is empty, clear the
            // override; otherwise set the new value. The daemon reloads-if-active.
            BackendMessage::BackendOptionSaved { source, name } => {
                self.action_error = None;
                let value = self
                    .backend_option_inputs
                    .get(&(source.clone(), name.clone()))
                    .cloned()
                    .unwrap_or_default();
                if value.is_empty() {
                    option_write(clear_backend_option(source, name))
                } else {
                    option_write(set_backend_option(source, name, value))
                }
            }

            // A switch writes on the press: there is no field to type into and
            // no Save to reach for, so the flip is the save.
            //
            // Flipping back to the manifest default clears the override rather
            // than storing a value identical to it, which keeps the daemon
            // config a record of real deviations and lets the backend's own
            // default move later without a stale copy pinning it.
            BackendMessage::BackendOptionToggled {
                source,
                name,
                value,
            } => {
                self.action_error = None;
                if self.option_bool_default(&source, &name) == Some(value) {
                    option_write(clear_backend_option(source, name))
                } else {
                    option_write(set_backend_option(source, name, value.to_string()))
                }
            }

            // Explicit reset: clear the stored override and reload so the
            // option reverts to its daemon default.
            BackendMessage::BackendOptionReset { source, name } => {
                self.action_error = None;
                option_write(clear_backend_option(source, name))
            }

            _ => Task::none(),
        }
    }
}
