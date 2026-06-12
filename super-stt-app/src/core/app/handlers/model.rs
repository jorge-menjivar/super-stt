// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, ModelOperationState};
use crate::daemon::client::{
    get_active_backend, get_current_device, get_current_model, get_gpu_info, list_available_models,
    list_backends, set_allow_online_models, set_model,
};
use crate::ui::messages::Message;
use cosmic::prelude::*;
use log::info;
use log::warn;
use super_stt_shared::models::provider::Provider;

impl AppModel {
    /// Handle model management messages
    pub(in crate::core::app) fn handle_model_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::LoadInitialData | Message::ModelSelected { .. } => {
                self.handle_model_load_commands(message)
            }

            Message::ModelsLoaded { .. }
            | Message::AvailableModelsLoaded(_)
            | Message::CurrentModelLoaded { .. }
            | Message::ModelChanged { .. }
            | Message::ModelError(_) => self.handle_model_results(message),

            _ => Task::none(),
        }
    }

    /// Handle model load commands: `LoadInitialData` and `ModelSelected`.
    fn handle_model_load_commands(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::LoadInitialData => {
                info!("LoadInitialData: Loading models and device info at startup");
                // One-time startup load: models + device info
                Task::batch([
                    Task::perform(list_available_models(), |result| match result {
                        Ok(models) => cosmic::Action::App(Message::AvailableModelsLoaded(models)),
                        Err(e) => cosmic::Action::App(Message::ModelError(e)),
                    }),
                    Task::perform(get_current_model(), |result| match result {
                        Ok((model, provider, source)) => {
                            cosmic::Action::App(Message::CurrentModelLoaded {
                                model,
                                provider,
                                source,
                            })
                        }
                        Err(e) => cosmic::Action::App(Message::ModelError(e)),
                    }),
                    Task::perform(get_current_device(), |result| match result {
                        Ok((device, available_devices, gpu_memory)) => {
                            info!(
                                "Initial device load successful: device={device}, available_devices={available_devices:?}"
                            );
                            cosmic::Action::App(Message::DeviceInfoLoaded(
                                device,
                                available_devices,
                                gpu_memory,
                            ))
                        }
                        Err(e) => {
                            warn!("Initial device load failed: {e}");
                            cosmic::Action::App(Message::DeviceError(e))
                        }
                    }),
                    // Online backends are gated by the install-time choice to
                    // add an online-capable backend, not a runtime toggle, so
                    // ensure the daemon permits them. Fire-and-forget.
                    Task::perform(set_allow_online_models(true), |_| cosmic::Action::None),
                    Task::perform(list_backends(), |result| match result {
                        Ok(backends) => cosmic::Action::App(Message::BackendsLoaded(backends)),
                        Err(e) => cosmic::Action::App(Message::BackendsError(e)),
                    }),
                    Task::perform(get_active_backend(), |result| {
                        cosmic::Action::App(Message::ActiveBackendLoaded(result.unwrap_or(None)))
                    }),
                    Task::perform(get_gpu_info(), |result| {
                        cosmic::Action::App(Message::GpuInfoLoaded(result.unwrap_or_default()))
                    }),
                ])
            }

            Message::ModelSelected {
                model,
                provider,
                source,
            } => {
                if model == self.current_model
                    && provider == self.current_provider
                    && source == self.current_source
                {
                    Task::none()
                } else {
                    // Atomic state check and transition to prevent race conditions
                    if !self.is_model_ready() {
                        warn!("Model operation already in progress - ignoring concurrent request");
                        return Task::none();
                    }

                    // Set loading state for the target model
                    self.set_model_loading(model.clone(), "Initiating model switch...".to_string());

                    let selected_model = model.clone();
                    let selected_source = source.clone();
                    let selected_provider = provider.clone();
                    Task::batch([
                        Task::perform(
                            set_model(model, provider, source),
                            move |result| match result {
                                Ok(_) => cosmic::Action::App(Message::ModelChanged {
                                    model: selected_model.clone(),
                                    provider: selected_provider.clone(),
                                    source: selected_source.clone(),
                                }),
                                Err(e) => cosmic::Action::App(Message::ModelError(e)),
                            },
                        ),
                        // Check download status immediately to see if download is needed
                        Task::perform(
                            async move {
                                // Small delay to allow daemon to start download if needed
                                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                            },
                            |()| cosmic::Action::App(Message::CheckDownloadStatus),
                        ),
                    ])
                }
            }

            _ => Task::none(),
        }
    }

    /// Handle model result messages: loads, changes, and errors.
    fn handle_model_results(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::ModelsLoaded {
                current_model,
                current_provider,
                current_source,
                available,
            } => {
                self.available_models = available;
                self.current_model = current_model;
                self.current_provider = current_provider;
                self.current_source = current_source;

                // Set model to ready state
                self.model_operation_state = ModelOperationState::Ready;

                Task::none()
            }

            Message::AvailableModelsLoaded(models) => {
                self.available_models = models;
                Task::none()
            }

            Message::CurrentModelLoaded {
                model,
                provider,
                source,
            }
            | Message::ModelChanged {
                model,
                provider,
                source,
            } => {
                self.current_model = model;
                self.current_provider = provider;
                self.current_source = source;
                self.model_operation_state = ModelOperationState::Ready;
                Task::none()
            }

            Message::ModelError(err) => {
                warn!("Model operation failed: {err}");
                let sanitized = err
                    .replace(&std::env::var("HOME").unwrap_or_default(), "$HOME")
                    .chars()
                    .take(200)
                    .collect::<String>();
                self.model_operation_state = ModelOperationState::Error { message: sanitized };
                // A failed switch leaves the daemon idle (no model) — the
                // backend stays selected, but no model is loaded.
                self.current_model = String::new();
                self.current_provider = Provider::default();
                self.current_source = String::new();
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
