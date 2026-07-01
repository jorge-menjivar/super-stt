// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::ui::messages::Message;
use cosmic::prelude::*;

impl AppModel {
    pub(in crate::core::app) fn handle_models_registry(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::RefreshRegistry => {
                // Always fetch the full annotated catalog; filtering is
                // client-side so the toggles never need a round-trip.
                let filters = crate::daemon::registry::ListFilters {
                    include_incompatible: Some(true),
                    ..Default::default()
                };
                Task::perform(
                    async move {
                        let _ = crate::daemon::registry::refresh().await;
                        crate::daemon::registry::list(&filters).await
                    },
                    |r| {
                        cosmic::Action::App(match r {
                            Ok(resp) => Message::RegistryListLoaded(resp),
                            Err(e) => Message::RegistryListFailed(e),
                        })
                    },
                )
            }

            Message::RegistryListLoaded(resp) => {
                self.registry.backends = resp.backends;
                self.registry.generated_at = Some(resp.generated_at);
                self.registry.last_refresh = Some(crate::state::registry::RefreshOutcome::Ok);
                Task::none()
            }

            Message::RegistryListFailed(err) => {
                self.registry.last_refresh =
                    Some(crate::state::registry::RefreshOutcome::Failed(err));
                Task::none()
            }

            Message::RegistrySearchChanged(s) => {
                self.registry.filters.search = s;
                Task::none()
            }

            Message::RegistryIncludeIncompatible(b) => {
                // The full catalog (incl. incompatible entries) is already
                // fetched; this is a pure client-side filter.
                self.registry.filters.include_incompatible = b;
                Task::none()
            }

            Message::RegistryOnlineFilter(o) => {
                self.registry.filters.online = o;
                Task::none()
            }

            Message::ImportBackendFromDir => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Import backend directory")
                        .pick_folder()
                        .await
                        .map(|h| h.path().to_string_lossy().into_owned())
                },
                |picked| cosmic::Action::App(Message::ImportBackendFromDirPicked(picked)),
            ),

            Message::ImportBackendFromDirPicked(picked) => {
                let Some(path) = picked else {
                    // User cancelled the picker — nothing to do.
                    return Task::none();
                };
                let key = path.clone();
                Task::perform(
                    async move { crate::daemon::registry::install_by_local_path(&path).await },
                    move |res| {
                        cosmic::Action::App(match res {
                            Ok(a) => Message::InstallAccepted {
                                source: key.clone(),
                                install_id: a.install_id,
                            },
                            Err(e) => Message::InstallFailedToStart {
                                source: key.clone(),
                                error: e,
                            },
                        })
                    },
                )
            }

            Message::RegistryCustomRepoInputChanged(s) => {
                self.registry.custom_repo_input = s;
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
