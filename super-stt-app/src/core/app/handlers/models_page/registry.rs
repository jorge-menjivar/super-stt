// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::ui::messages::{Message, ModelsPageMessage};
use cosmic::prelude::*;

impl AppModel {
    pub(in crate::core::app) fn handle_models_registry(
        &mut self,
        message: ModelsPageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            ModelsPageMessage::RefreshRegistry => {
                // Refresh the index, then fetch the full annotated catalog;
                // filtering is client-side so the toggles never need a round-trip.
                crate::core::app::handlers::tasks::fetch_registry_catalog(true)
            }

            ModelsPageMessage::RegistryListLoaded(resp) => {
                self.registry.backends = resp.backends;
                self.registry.generated_at = Some(resp.generated_at);
                self.registry.last_refresh = Some(crate::state::registry::RefreshOutcome::Ok);
                Task::none()
            }

            ModelsPageMessage::RegistryListFailed(err) => {
                self.registry.last_refresh =
                    Some(crate::state::registry::RefreshOutcome::Failed(err));
                Task::none()
            }

            ModelsPageMessage::RegistrySearchChanged(s) => {
                self.registry.filters.search = s;
                Task::none()
            }

            ModelsPageMessage::RegistryIncludeIncompatible(b) => {
                // The full catalog (incl. incompatible entries) is already
                // fetched; this is a pure client-side filter.
                self.registry.filters.include_incompatible = b;
                Task::none()
            }

            ModelsPageMessage::RegistryOnlineFilter(o) => {
                self.registry.filters.online = o;
                Task::none()
            }

            ModelsPageMessage::ImportBackendFromDir => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Import backend directory")
                        .pick_folder()
                        .await
                        .map(|h| h.path().to_string_lossy().into_owned())
                },
                |picked| {
                    cosmic::Action::App(Message::ModelsPage(
                        ModelsPageMessage::ImportBackendFromDirPicked(picked),
                    ))
                },
            ),

            ModelsPageMessage::ImportBackendFromDirPicked(picked) => {
                let Some(path) = picked else {
                    // User cancelled the picker — nothing to do.
                    return Task::none();
                };
                let key = path.clone();
                Task::perform(
                    async move { crate::daemon::registry::install_by_local_path(&path).await },
                    move |res| {
                        cosmic::Action::App(Message::ModelsPage(match res {
                            Ok(a) => ModelsPageMessage::InstallAccepted {
                                source: key.clone(),
                                install_id: a.install_id,
                            },
                            Err(e) => ModelsPageMessage::InstallFailedToStart {
                                source: key.clone(),
                                error: e.to_string(),
                            },
                        }))
                    },
                )
            }

            ModelsPageMessage::RegistryCustomRepoInputChanged(s) => {
                self.registry.custom_repo_input = s;
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
