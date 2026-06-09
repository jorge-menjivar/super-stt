// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::{list_backends, reload_active_model, set_backend_option};
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
            Message::BackendsLoaded(_) | Message::BackendsError(_) | Message::BackendsReload => {
                self.handle_backend_catalog(message)
            }

            Message::BackendSecretInputChanged { .. }
            | Message::BackendSecretSaved { .. }
            | Message::BackendSecretRemoved { .. }
            | Message::BackendOptionInputChanged { .. }
            | Message::BackendOptionSaved { .. } => self.handle_backend_config(message),

            _ => Task::none(),
        }
    }

    /// Handle backend catalog messages: `BackendsLoaded`, `BackendsError`, `BackendsReload`.
    fn handle_backend_catalog(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::BackendsLoaded(backends) => {
                // Prefill option input buffers from each option's current
                // value, and recompute which secrets are configured by
                // probing the keyring. Both maps are keyed by (source, name).
                self.backend_option_inputs.clear();
                self.backend_secret_configured.clear();
                for backend in &backends {
                    for option in &backend.options {
                        self.backend_option_inputs.insert(
                            (backend.source.clone(), option.name.clone()),
                            option.value.clone().unwrap_or_default(),
                        );
                    }
                    for secret in &backend.secrets {
                        let configured =
                            crate::keyring::has_backend_secret(&backend.source, &secret.name)
                                .unwrap_or(false);
                        self.backend_secret_configured
                            .insert((backend.source.clone(), secret.name.clone()), configured);
                    }
                }
                self.backends = backends;
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

            // Keyring writes are synchronous and same-machine, so we
            // perform them inline and update `backend_secret_configured`
            // directly rather than round-tripping through another message.
            Message::BackendSecretSaved { source, name } => {
                let key = (source.clone(), name.clone());
                let value = self
                    .backend_secret_inputs
                    .get(&key)
                    .cloned()
                    .unwrap_or_default();
                if value.is_empty() {
                    return Task::none();
                }
                match crate::keyring::set_backend_secret(&source, &name, &value) {
                    Ok(()) => {
                        self.backend_secret_configured.insert(key.clone(), true);
                        self.backend_secret_inputs.remove(&key);
                    }
                    Err(e) => log::warn!("Failed to store secret {source}:{name}: {e}"),
                }
                self.reload_if_active_backend(&source)
            }

            Message::BackendSecretRemoved { source, name } => {
                let key = (source.clone(), name.clone());
                match crate::keyring::delete_backend_secret(&source, &name) {
                    Ok(()) => {
                        self.backend_secret_configured.insert(key.clone(), false);
                        self.backend_secret_inputs.remove(&key);
                    }
                    Err(e) => log::warn!("Failed to delete secret {source}:{name}: {e}"),
                }
                self.reload_if_active_backend(&source)
            }

            Message::BackendOptionInputChanged {
                source,
                name,
                value,
            } => {
                self.backend_option_inputs.insert((source, name), value);
                Task::none()
            }

            // Options live in the daemon config, so this dispatches an
            // async client call and reloads the catalog on success to
            // reflect the new effective value.
            Message::BackendOptionSaved { source, name } => {
                let value = self
                    .backend_option_inputs
                    .get(&(source.clone(), name.clone()))
                    .cloned()
                    .unwrap_or_default();
                Task::perform(
                    set_backend_option(source, name, value),
                    |result| match result {
                        Ok(_) => cosmic::Action::App(Message::BackendsReload),
                        Err(e) => cosmic::Action::App(Message::BackendsError(e)),
                    },
                )
            }

            _ => Task::none(),
        }
    }

    /// If `source` serves the currently-active model, reload it so a
    /// just-changed secret/option takes effect (secrets/options are read at
    /// load time). Otherwise a no-op.
    pub(in crate::core::app) fn reload_if_active_backend(
        &self,
        source: &str,
    ) -> Task<cosmic::Action<Message>> {
        if source == self.current_source {
            Task::perform(reload_active_model(), |result| match result {
                Ok(_) => cosmic::Action::App(Message::BackendsReload),
                Err(e) => cosmic::Action::App(Message::BackendsError(e)),
            })
        } else {
            Task::none()
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
