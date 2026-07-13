// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::list_backends;
use crate::ui::messages::Message;
use cosmic::prelude::*;

impl AppModel {
    pub(in crate::core::app) fn handle_models_install_lifecycle(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Registry / Download-tab messages
            Message::InstallBackend(source) => {
                // Clear a prior "failed to start" so the card reads "Installing…".
                self.registry.install_errors.remove(&source);
                let s = source.clone();
                Task::perform(
                    async move { crate::daemon::registry::install_by_source(&s).await },
                    move |res| {
                        cosmic::Action::App(match res {
                            Ok(a) => Message::InstallAccepted {
                                source: source.clone(),
                                install_id: a.install_id,
                            },
                            Err(e) => Message::InstallFailedToStart {
                                source: source.clone(),
                                error: e.to_string(),
                            },
                        })
                    },
                )
            }

            Message::InstallBackendFromRepoUrl(url) => {
                self.registry.install_errors.remove(&url);
                let u = url.clone();
                Task::perform(
                    async move { crate::daemon::registry::install_by_repo_url(&u).await },
                    move |res| {
                        cosmic::Action::App(match res {
                            Ok(a) => Message::InstallAccepted {
                                source: url.clone(),
                                install_id: a.install_id,
                            },
                            Err(e) => Message::InstallFailedToStart {
                                source: url.clone(),
                                error: e.to_string(),
                            },
                        })
                    },
                )
            }

            Message::InstallAccepted { source, install_id } => {
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

            Message::InstallFailedToStart { source, error } => {
                log::error!("install({source}) failed to start: {error}");
                // Drop the pending marker (there is no background install) and
                // record the reason so the Browse card shows "Failed" + a note
                // instead of silently snapping back to the Install button.
                self.registry.installs.remove(&source);
                self.registry.install_errors.insert(source, error);
                Task::none()
            }

            Message::UpdateBackend(source) => {
                self.registry.install_errors.remove(&source);
                let s = source.clone();
                Task::perform(
                    async move { crate::daemon::registry::update(&s).await },
                    move |res| {
                        cosmic::Action::App(match res {
                            Ok(r) if r.noop => {
                                log::info!("update({source}) noop — already at {}", r.to_version);
                                // Nothing to do; let the UI settle naturally.
                                Message::BackendsReload
                            }
                            Ok(r) => Message::InstallAccepted {
                                source: source.clone(),
                                install_id: r.install_id.unwrap_or_default(),
                            },
                            Err(e) => Message::InstallFailedToStart {
                                source: source.clone(),
                                error: e.to_string(),
                            },
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
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::UninstallBackend(source) => {
                // Clear any stale failure so the row reads "in progress" on retry.
                self.registry.uninstall_errors.remove(&source);
                let s = source.clone();
                Task::perform(
                    async move { crate::daemon::registry::uninstall(&s).await },
                    move |res| {
                        cosmic::Action::App(match res {
                            Ok(_) => Message::BackendsReload,
                            Err(error) => Message::UninstallFailed {
                                source: source.clone(),
                                error: error.to_string(),
                            },
                        })
                    },
                )
            }

            Message::UninstallFailed { source, error } => {
                log::error!("uninstall({source}) failed: {error}");
                self.registry.uninstall_errors.insert(source, error);
                Task::none()
            }

            _ => Task::none(),
        }
    }

    pub(in crate::core::app) fn handle_models_install_progress(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::InstallProgress {
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

            Message::InstallCompleted { source } => {
                self.registry.installs.remove(&source);
                self.registry.install_errors.remove(&source);
                // Refresh the installed-backends list so the new install
                // shows up in the Installed tab.
                Task::perform(list_backends(), |result| match result {
                    Ok(backends) => cosmic::Action::App(Message::BackendsLoaded(backends)),
                    Err(e) => cosmic::Action::App(Message::BackendsError(e.to_string())),
                })
            }

            Message::InstallFailed {
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
