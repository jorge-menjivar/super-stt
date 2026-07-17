// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, DeviceState, ModelOperationState};
use crate::ui::messages::{DaemonMessage, DeviceMessage, DownloadMessage, Message};
use cosmic::prelude::*;
use log::{debug, info, warn};
use super_stt_shared::models::protocol::{DaemonStatusEvent, NotificationEvent};
use super_stt_shared::models::provider::Provider;

impl AppModel {
    pub(in crate::core::app) fn handle_daemon_events(
        &mut self,
        message: DaemonMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            DaemonMessage::DaemonEventsReceived(events) => {
                debug!("Received {} daemon events", events.len());
                // Process EVERY event and collect their tasks. Returning on the
                // first event that yields a task dropped the rest of the batch
                // (and left `last_event_timestamp` stuck before them) — latent
                // only because producers currently wrap singletons (Tier 1 #17).
                let mut tasks = Vec::new();
                for event in events {
                    // Update timestamp for next polling
                    self.last_event_timestamp = Some(event.timestamp.clone());

                    if event.event_type == "daemon_status_changed" {
                        if let Some(task) = self.process_daemon_status_event(&event) {
                            tasks.push(task);
                        }
                    } else if event.event_type == "download_progress"
                        && let Some(task) = self.process_download_progress_event(&event)
                    {
                        tasks.push(task);
                    }
                }
                // Force UI update after processing events that may change state
                tasks.push(self.update_title());
                Task::batch(tasks)
            }

            _ => Task::none(),
        }
    }
    // ^ `handle_daemon_events` only ever receives `DaemonEventsReceived`, but it
    // takes the full `DaemonMessage` for a uniform delegate signature; the
    // catch-all covers the other (unreachable) variants without a panic.

    pub(in crate::core::app) fn process_daemon_status_event(
        &mut self,
        event: &NotificationEvent,
    ) -> Option<Task<cosmic::Action<Message>>> {
        info!("Received daemon event: {:?}", event.data);
        // Typed deserialize replaces the former hand-matched `.get("status")` /
        // `.get("<field>")` reads: a field rename/typo is now a compile error, not
        // a silent no-op (audit 2 Tier 2 #9). The injected `timestamp` key is
        // ignored; an unrecognized or malformed event yields `None`.
        let event = match serde_json::from_value::<DaemonStatusEvent>(event.data.clone()) {
            Ok(ev) => ev,
            Err(e) => {
                info!("Unhandled/unparseable daemon status event: {e}");
                return None;
            }
        };
        match event {
            DaemonStatusEvent::Ready {
                model_loaded,
                actual_device,
                ..
            } => {
                self.handle_daemon_ready(actual_device.as_deref(), model_loaded);
                None
            }
            DaemonStatusEvent::DeviceSwitchError { error, .. } => {
                Some(self.handle_daemon_device_error(&error))
            }
            DaemonStatusEvent::ModelSwitched {
                model_name,
                provider,
                source,
                ..
            } => Some(self.handle_daemon_model_switched(&model_name, &provider, &source)),
            DaemonStatusEvent::SwitchingDevice { target_device, .. } => {
                info!("Received switching_device event -> {target_device}");
                // Keep device_state as Switching and wait for the `ready` event;
                // this event just confirms the switch is in progress.
                if !matches!(self.device_state, DeviceState::Switching { .. }) {
                    warn!("Received switching_device event but not in switching state");
                    self.set_device_switching(target_device, "Switching device...".to_string());
                }
                None
            }
            DaemonStatusEvent::LoadingModelForDevice {
                model,
                target_device,
            } => {
                info!("Received loading_model_for_device event: {model} on {target_device}");
                let status_message = format!(
                    "Loading {} on {}...",
                    model,
                    if target_device == "cpu" { "CPU" } else { "GPU" }
                );
                self.set_device_switching(target_device, status_message);
                None
            }
            DaemonStatusEvent::SettingsChanged { setting } => {
                // A setting changed — possibly from another client, or the global
                // Primary Language this very client just set. Re-fetch the language
                // state so a per-model button that follows the global value, and
                // the global card, reflect the new value.
                if setting != "language" {
                    return None;
                }
                let mut tasks = vec![self.load_primary_language()];
                if let Some((source, model)) = self.language.model_language_for.clone() {
                    tasks.push(self.load_model_language(source, model));
                }
                Some(Task::batch(tasks))
            }
            // No app-side effect today (matches the prior `_` fall-through): the
            // load-start and active-backend-changed notifications don't drive UI
            // state here.
            DaemonStatusEvent::LoadingModel { .. }
            | DaemonStatusEvent::ActiveBackendChanged { .. } => None,
        }
    }

    pub(in crate::core::app) fn handle_daemon_ready(
        &mut self,
        actual_device: Option<&str>,
        model_loaded: bool,
    ) {
        // Handle device readiness
        if let Some(actual_device) = actual_device {
            info!(
                "Received ready event: current_device={} -> {}",
                self.current_device, actual_device
            );
            self.current_device = actual_device.to_string();

            // If we were switching devices, this marks completion
            if matches!(self.device_state, DeviceState::Switching { .. }) {
                info!("Device switch completed to: {actual_device}");
            }
            self.device_state = DeviceState::Ready;
        }

        // Handle model readiness - clear switching state
        if model_loaded {
            info!("Received ready event: model loading completed");
            info!(
                "Model state before ready event: {:?}",
                self.model_operation_state
            );
            self.model_operation_state = ModelOperationState::Ready;
            info!(
                "Model state after ready event: {:?}",
                self.model_operation_state
            );
        }
    }

    pub(in crate::core::app) fn handle_daemon_device_error(
        &mut self,
        error: &str,
    ) -> Task<cosmic::Action<Message>> {
        warn!("Received device switch error event: {error}");
        // Reset device state from switching to ready
        if matches!(self.device_state, DeviceState::Switching { .. }) {
            info!("Device switch failed, reverting to ready state");
        }
        self.device_state = DeviceState::Ready;
        let error_message = error.to_string();
        // Show error to user
        Task::perform(async move { error_message }, |msg| {
            cosmic::Action::App(Message::Device(DeviceMessage::DeviceError(msg)))
        })
    }

    pub(in crate::core::app) fn handle_daemon_model_switched(
        &mut self,
        model_name: &str,
        provider: &str,
        source: &str,
    ) -> Task<cosmic::Action<Message>> {
        let model = model_name.to_string();
        // The wire carries the provider's `Display` form; fall back to the current
        // provider if it doesn't parse (forward-compat with an unknown provider).
        let provider = provider
            .parse::<Provider>()
            .unwrap_or_else(|_| self.current_provider.clone());
        let source = source.to_string();
        info!(
            "Received model_switched event: current_model={:?} -> {:?} via {provider} ({source})",
            self.current_model, model
        );
        // A live identity change supersedes any in-flight reconnect snapshot: bump
        // the epoch so a stale get_current_model response can't revert this.
        self.current_model_epoch = self.current_model_epoch.wrapping_add(1);
        self.current_model.clone_from(&model);
        self.current_provider = provider;
        self.current_source.clone_from(&source);
        self.model_operation_state = ModelOperationState::Ready;
        info!("Model state updated to Ready after model_switched event");
        // Mirror CurrentModelLoaded/ModelChanged: fetch the per-model language
        // block so the active-backend card's language button shows the model's
        // resolved language instead of the neutral "Language" label. A client that
        // learns the active model only via this broadcast — e.g. the settings app
        // reconnecting after a daemon restart, where the startup load now emits
        // model_switched — would otherwise leave model_language_for unset.
        self.load_model_language(source, model)
    }

    pub(in crate::core::app) fn process_download_progress_event(
        &mut self,
        event: &NotificationEvent,
    ) -> Option<Task<cosmic::Action<Message>>> {
        // Handle download progress events
        let progress =
            serde_json::from_value::<super_stt_shared::models::protocol::DownloadProgress>(
                event.data.clone(),
            )
            .ok()?;

        debug!(
            "Received download progress event: {}% for {}",
            progress.percentage, progress.model_name
        );
        self.apply_download_progress(&progress);

        // Handle download completion/failure. The `error` case is fully
        // handled by `apply_download_progress` above (it sets the Error banner
        // with the daemon's failure detail and clears the selection), so it
        // needs no follow-up message here — emitting a generic `DownloadError`
        // would only reset the state and overwrite the detailed message.
        if progress.status == "completed" {
            // Send download completed message; model_switched event from the daemon
            // will update state if needed — no explicit reload required.
            return Some(Task::perform(async move { progress.model_name }, |model| {
                cosmic::Action::App(Message::Download(DownloadMessage::DownloadCompleted(model)))
            }));
        } else if progress.status == "cancelled" {
            return Some(Task::perform(async move { progress.model_name }, |model| {
                cosmic::Action::App(Message::Download(DownloadMessage::DownloadCancelled(model)))
            }));
        }
        None
    }
}
