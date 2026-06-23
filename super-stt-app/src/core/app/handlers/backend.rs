// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::{
    clear_backend_option, clear_backend_secret, list_backend_secrets, list_backends,
    set_backend_option, set_backend_secret,
};
use crate::ui::messages::Message;
use cosmic::prelude::*;
use super_stt_shared::models::provider::Provider;

impl AppModel {
    /// Backend catalog refresh + per-backend secret/option configuration
    /// (used by the Configure sub-view).
    pub(in crate::core::app) fn handle_backend_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::BackendsLoaded(_)
            | Message::BackendsError(_)
            | Message::BackendsReload
            | Message::BackendSecretsConfigured { .. } => self.handle_backend_catalog(message),

            Message::BackendSecretInputChanged { .. }
            | Message::BackendSecretSaved { .. }
            | Message::BackendSecretStored { .. }
            | Message::BackendSecretRemoved { .. }
            | Message::BackendOptionInputChanged { .. }
            | Message::BackendOptionSaved { .. } => self.handle_backend_config(message),

            _ => Task::none(),
        }
    }

    /// Handle backend catalog messages: `BackendsLoaded`, `BackendsError`, `BackendsReload`,
    /// `BackendSecretsConfigured`.
    fn handle_backend_catalog(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::BackendsLoaded(backends) => {
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
                let tasks: Vec<_> = backends
                    .iter()
                    .map(|b| {
                        let source = b.source.clone();
                        Task::perform(list_backend_secrets(source.clone()), move |res| {
                            let items = res.unwrap_or_default();
                            cosmic::Action::App(Message::BackendSecretsConfigured {
                                source: source.clone(),
                                items,
                            })
                        })
                    })
                    .collect();
                self.backends = backends;
                Task::batch(tasks)
            }

            Message::BackendSecretsConfigured { source, items } => {
                for (name, configured) in items {
                    self.backend_secret_configured
                        .insert((source.clone(), name), configured);
                }
                Task::none()
            }

            Message::BackendsError(err) => {
                log::warn!("Backends load error: {err}");
                Task::none()
            }

            Message::BackendsReload => Task::perform(list_backends(), |result| match result {
                Ok(backends) => cosmic::Action::App(Message::BackendsLoaded(backends)),
                Err(e) => cosmic::Action::App(Message::BackendsError(e)),
            }),

            _ => Task::none(),
        }
    }

    /// Handle per-backend secret and option configuration messages.
    fn handle_backend_config(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::BackendSecretInputChanged {
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
            Message::BackendSecretSaved { source, name } => {
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
                        Ok(()) => cosmic::Action::App(Message::BackendSecretStored {
                            source: source.clone(),
                            name: name.clone(),
                        }),
                        Err(e) => cosmic::Action::App(Message::BackendsError(e)),
                    },
                )
            }

            // Daemon confirmed the secret was written — clear the input buffer
            // and refresh the catalog so the configured flag updates.
            Message::BackendSecretStored { source, name } => {
                self.backend_secret_inputs.remove(&(source, name));
                self.handle_backend_catalog(Message::BackendsReload)
            }

            // Clear secret via daemon; daemon reloads-if-active itself.
            Message::BackendSecretRemoved { source, name } => {
                self.backend_secret_inputs
                    .remove(&(source.clone(), name.clone()));
                Task::perform(clear_backend_secret(source, name), move |res| match res {
                    Ok(()) => cosmic::Action::App(Message::BackendsReload),
                    Err(e) => cosmic::Action::App(Message::BackendsError(e)),
                })
            }

            Message::BackendOptionInputChanged {
                source,
                name,
                value,
            } => {
                self.backend_option_inputs.insert((source, name), value);
                Task::none()
            }

            // Options go to the daemon config. If the input is empty, clear the
            // override; otherwise set the new value. The daemon reloads-if-active.
            Message::BackendOptionSaved { source, name } => {
                let value = self
                    .backend_option_inputs
                    .get(&(source.clone(), name.clone()))
                    .cloned()
                    .unwrap_or_default();
                if value.is_empty() {
                    Task::perform(clear_backend_option(source, name), |result| match result {
                        Ok(()) => cosmic::Action::App(Message::BackendsReload),
                        Err(e) => cosmic::Action::App(Message::BackendsError(e)),
                    })
                } else {
                    Task::perform(
                        set_backend_option(source, name, value),
                        |result| match result {
                            Ok(()) => cosmic::Action::App(Message::BackendsReload),
                            Err(e) => cosmic::Action::App(Message::BackendsError(e)),
                        },
                    )
                }
            }

            _ => Task::none(),
        }
    }

    /// Look up the [`Provider`] for a `(source, model)` pair against the
    /// installed-backend catalog. Returns `None` if the source isn't
    /// installed, doesn't serve that model, or its `provider` string fails to
    /// parse — the daemon will reject any of these cases anyway, so silently
    /// dropping the request is correct.
    pub(in crate::core::app) fn backend_model_provider(
        &self,
        source: &str,
        model: &str,
    ) -> Option<Provider> {
        self.backends
            .iter()
            .find(|b| b.source == source)?
            .models
            .iter()
            .find(|m| m.name == model)?
            .provider
            .parse::<Provider>()
            .ok()
    }
}
