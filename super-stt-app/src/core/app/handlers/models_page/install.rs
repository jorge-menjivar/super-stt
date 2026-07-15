// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::ui::messages::{BackendMessage, Message, ModelsPageMessage};
use cosmic::prelude::*;

impl AppModel {
    pub(in crate::core::app) fn handle_models_install_lifecycle(
        &mut self,
        message: ModelsPageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Registry / Download-tab messages
            ModelsPageMessage::InstallBackend(source) => {
                // Clear a prior "failed to start" so the card reads "Installing…".
                self.registry.install_errors.remove(&source);
                let s = source.clone();
                Task::perform(
                    async move { crate::daemon::registry::install_by_source(&s).await },
                    move |res| {
                        cosmic::Action::App(Message::ModelsPage(match res {
                            Ok(a) => ModelsPageMessage::InstallAccepted {
                                source: source.clone(),
                                install_id: a.install_id,
                            },
                            Err(e) => ModelsPageMessage::InstallFailedToStart {
                                source: source.clone(),
                                error: e.to_string(),
                            },
                        }))
                    },
                )
            }

            ModelsPageMessage::InstallBackendFromRepoUrl(url) => {
                self.registry.install_errors.remove(&url);
                let u = url.clone();
                Task::perform(
                    async move { crate::daemon::registry::install_by_repo_url(&u).await },
                    move |res| {
                        cosmic::Action::App(Message::ModelsPage(match res {
                            Ok(a) => ModelsPageMessage::InstallAccepted {
                                source: url.clone(),
                                install_id: a.install_id,
                            },
                            Err(e) => ModelsPageMessage::InstallFailedToStart {
                                source: url.clone(),
                                error: e.to_string(),
                            },
                        }))
                    },
                )
            }

            ModelsPageMessage::InstallAccepted { source, install_id } => {
                use crate::state::registry::InstallStatus;
                use super_stt_shared::registry::events::InstallPhase;
                // The request was accepted — retire any prior start error.
                self.registry.install_errors.remove(&source);
                self.registry.installs.insert(
                    source,
                    InstallStatus {
                        install_id,
                        phase: InstallPhase::Downloading,
                        bytes_done: 0,
                        bytes_total: None,
                        error: None,
                    },
                );
                Task::none()
            }

            ModelsPageMessage::InstallFailedToStart { source, error } => {
                log::error!("install({source}) failed to start: {error}");
                // Drop the pending marker (there is no background install) and
                // record the reason so the Browse card shows "Failed" + a note
                // instead of silently snapping back to the Install button.
                self.registry.installs.remove(&source);
                self.registry.install_errors.insert(source, error);
                Task::none()
            }

            ModelsPageMessage::UpdateBackend(source) => {
                self.registry.install_errors.remove(&source);
                let s = source.clone();
                Task::perform(
                    async move { crate::daemon::registry::update(&s).await },
                    move |res| {
                        cosmic::Action::App(match res {
                            Ok(r) if r.noop => {
                                log::info!("update({source}) noop — already at {}", r.to_version);
                                // Nothing to do; let the UI settle naturally.
                                Message::Backend(BackendMessage::BackendsReload)
                            }
                            Ok(r) => Message::ModelsPage(ModelsPageMessage::InstallAccepted {
                                source: source.clone(),
                                install_id: r.install_id.unwrap_or_default(),
                            }),
                            Err(e) => {
                                Message::ModelsPage(ModelsPageMessage::InstallFailedToStart {
                                    source: source.clone(),
                                    error: e.to_string(),
                                })
                            }
                        })
                    },
                )
            }

            _ => Task::none(),
        }
    }

    /// Uninstall lifecycle: kick off the request and surface its failure.
    pub(in crate::core::app) fn handle_models_uninstall(
        &mut self,
        message: ModelsPageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            ModelsPageMessage::UninstallBackend(source) => {
                // Clear any stale failure so the row reads "in progress" on retry.
                self.registry.uninstall_errors.remove(&source);
                let s = source.clone();
                Task::perform(
                    async move { crate::daemon::registry::uninstall(&s).await },
                    move |res| {
                        cosmic::Action::App(match res {
                            Ok(_) => Message::Backend(BackendMessage::BackendsReload),
                            Err(error) => Message::ModelsPage(ModelsPageMessage::UninstallFailed {
                                source: source.clone(),
                                error: error.to_string(),
                            }),
                        })
                    },
                )
            }

            ModelsPageMessage::UninstallFailed { source, error } => {
                log::error!("uninstall({source}) failed: {error}");
                self.registry.uninstall_errors.insert(source, error);
                Task::none()
            }

            _ => Task::none(),
        }
    }

    pub(in crate::core::app) fn handle_models_install_progress(
        &mut self,
        message: ModelsPageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            ModelsPageMessage::InstallProgress {
                install_id,
                source,
                phase,
                bytes_done,
                bytes_total,
            } => {
                if let Some(s) = self.registry.installs.get_mut(&source)
                    && s.install_id == install_id
                {
                    s.phase = phase;
                    if let Some(d) = bytes_done {
                        s.bytes_done = d;
                    }
                    if let Some(t) = bytes_total {
                        s.bytes_total = Some(t);
                    }
                }
                Task::none()
            }

            ModelsPageMessage::InstallCompleted { source } => {
                self.registry.installs.remove(&source);
                self.registry.install_errors.remove(&source);
                // Refresh the installed-backends list so the new install
                // shows up in the Installed tab.
                crate::core::app::handlers::tasks::reload_backends()
            }

            ModelsPageMessage::InstallFailed {
                install_id,
                source,
                phase,
                error,
            } => {
                if let Some(s) = self.registry.installs.get_mut(&source)
                    && s.install_id == install_id
                {
                    s.error = Some(error);
                    s.phase = phase;
                }
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
