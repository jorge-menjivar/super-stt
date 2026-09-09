// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::ui::messages::{Message, ModelsPageMessage};
use cosmic::prelude::*;

/// Open the folder picker. Shared by the drawer's Choose button and by Check
/// pressed with no folder chosen yet, which mean the same thing.
fn pick_folder_task() -> Task<cosmic::Action<Message>> {
    Task::perform(
        async {
            rfd::AsyncFileDialog::new()
                .set_title("Choose a backend folder")
                .pick_folder()
                .await
                .map(|h| h.path().to_string_lossy().into_owned())
        },
        |picked| {
            cosmic::Action::App(Message::ModelsPage(
                ModelsPageMessage::ImportBackendFromDirPicked(picked),
            ))
        },
    )
}

/// Map a preview call's outcome onto the two messages that render it. Both
/// land in the same panel, which is the point: a failed resolve is as visible
/// as a successful one.
fn preview_outcome(
    res: super_stt_shared::daemon::http_client::HttpResult<
        super_stt_shared::registry::PreviewResponse,
    >,
) -> cosmic::Action<Message> {
    cosmic::Action::App(Message::ModelsPage(match res {
        Ok(r) => ModelsPageMessage::AddPreviewLoaded(Box::new(r)),
        Err(e) => {
            // The panel is read by whoever just pasted a URL, so it gets the
            // daemon's sentence. The log keeps the token and status, which is
            // what made issue #423 diagnosable from a log alone.
            log::error!("preview failed: {e}");
            ModelsPageMessage::AddPreviewFailed(e.user_message())
        }
    }))
}

impl AppModel {
    pub(in crate::core::app) fn handle_models_registry(
        &mut self,
        message: ModelsPageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            ModelsPageMessage::RefreshRegistry => {
                // Reachable from the toolbar menu, which has to go once it has
                // been used: a menu left standing over the list it just acted
                // on reads as though the click missed.
                self.models_page.browse_menu_open = false;
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
            ModelsPageMessage::RegistryRoleFilter(r) => {
                self.registry.filters.role = r;
                Task::none()
            }
            ModelsPageMessage::InstalledOnlineFilter(o) => {
                self.models_page.installed_filters.online = o;
                Task::none()
            }
            ModelsPageMessage::InstalledRoleFilter(r) => {
                self.models_page.installed_filters.role = r;
                Task::none()
            }

            ModelsPageMessage::ImportBackendFromDir => pick_folder_task(),

            ModelsPageMessage::ImportBackendFromDirPicked(picked) => {
                let Some(path) = picked else {
                    // User cancelled the picker — nothing to do.
                    return Task::none();
                };
                // Choosing a folder is the whole gesture, so it resolves right
                // away: there is nothing further to type and no network call to
                // hold back. A URL needs an explicit Check for both reasons.
                self.registry.add_folder.clone_from(&path);
                self.registry.add_preview =
                    crate::state::registry::PreviewState::Checking(path.clone());
                Task::perform(
                    async move { crate::daemon::registry::preview_by_local_path(&path).await },
                    preview_outcome,
                )
            }

            ModelsPageMessage::RegistryCustomRepoInputChanged(s) => {
                // Editing the URL retires the preview beside it: it described
                // whatever was in the box a moment ago, and leaving it up would
                // let Install commit to a backend the field no longer names.
                if self.registry.custom_repo_input != s {
                    self.registry.add_preview = crate::state::registry::PreviewState::Empty;
                }
                self.registry.custom_repo_input = s;
                Task::none()
            }

            ModelsPageMessage::ToggleBrowseMenu => {
                self.models_page.browse_menu_open = !self.models_page.browse_menu_open;
                Task::none()
            }

            ModelsPageMessage::CloseBrowseMenu => {
                self.models_page.browse_menu_open = false;
                Task::none()
            }

            _ => Task::none(),
        }
    }
}

impl AppModel {
    /// The Add-a-backend drawer: pick a source, resolve it, commit to it.
    ///
    /// Kept apart from [`AppModel::handle_models_registry`] because it is a
    /// flow with its own state (source, folder, preview) rather than another
    /// catalog action.
    pub(in crate::core::app) fn handle_add_sheet(
        &mut self,
        message: ModelsPageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            ModelsPageMessage::AddFolderInputChanged(path) => {
                // Same rule as the URL field: editing retires the preview
                // beside it, so Install can never commit to a folder the field
                // no longer names.
                if self.registry.add_folder != path {
                    self.registry.add_preview = crate::state::registry::PreviewState::Empty;
                }
                self.registry.add_folder = path;
                Task::none()
            }

            ModelsPageMessage::AddSourceChanged(src) => {
                if self.registry.add_source != src {
                    self.registry.add_source = src;
                    // The preview described the other source.
                    self.registry.add_preview = crate::state::registry::PreviewState::Empty;
                }
                Task::none()
            }

            ModelsPageMessage::CheckAddSource => {
                use crate::state::registry::{AddSource, PreviewState};
                match self.registry.add_source {
                    AddSource::Repository => {
                        let url = self.registry.custom_repo_input.trim().to_string();
                        if url.is_empty() {
                            return Task::none();
                        }
                        self.registry.add_preview = PreviewState::Checking(url.clone());
                        Task::perform(
                            async move { crate::daemon::registry::preview_by_repo_url(&url).await },
                            preview_outcome,
                        )
                    }
                    AddSource::Folder => {
                        let path = self.registry.add_folder.trim().to_string();
                        if path.is_empty() {
                            // Nothing typed and nothing picked: offer the picker.
                            return pick_folder_task();
                        }
                        self.registry.add_preview = PreviewState::Checking(path.clone());
                        Task::perform(
                            async move { crate::daemon::registry::preview_by_local_path(&path).await },
                            preview_outcome,
                        )
                    }
                }
            }

            ModelsPageMessage::AddPreviewLoaded(resp) => {
                self.registry.add_preview =
                    crate::state::registry::PreviewState::Ready(Box::new(resp.backend));
                Task::none()
            }

            ModelsPageMessage::AddPreviewFailed(err) => {
                self.registry.add_preview = crate::state::registry::PreviewState::Failed(err);
                Task::none()
            }

            ModelsPageMessage::AddPreviewSpin => {
                // Wrapping rather than saturating: a check long enough to reach
                // usize::MAX frames is not a thing, but a spinner that stops
                // turning would be a strange way to find that out.
                self.registry.add_spin = self.registry.add_spin.wrapping_add(1);
                Task::none()
            }

            ModelsPageMessage::InstallPreviewed => {
                use crate::state::registry::AddSource;
                // Install what was previewed, keyed by what the user pointed at
                // — the same string the preview resolved and the same key the
                // install events will carry.
                let key = self.registry.add_key();
                if key.is_empty() {
                    return Task::none();
                }
                self.registry.install_errors.remove(&key);
                let is_folder = self.registry.add_source == AddSource::Folder;
                let (k1, k2) = (key.clone(), key.clone());
                Task::perform(
                    async move {
                        if is_folder {
                            crate::daemon::registry::install_by_local_path(&key).await
                        } else {
                            crate::daemon::registry::install_by_repo_url(&key).await
                        }
                    },
                    move |res| {
                        cosmic::Action::App(Message::ModelsPage(match res {
                            Ok(a) => ModelsPageMessage::InstallAccepted {
                                source: k1.clone(),
                                install_id: a.install_id,
                            },
                            Err(e) => ModelsPageMessage::InstallFailedToStart {
                                source: k2.clone(),
                                error: e.to_string(),
                            },
                        }))
                    },
                )
            }

            _ => Task::none(),
        }
    }
}
