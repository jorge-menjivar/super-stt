// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, DeviceState, ModelOperationState};
use crate::ui::messages::Message;
use cosmic::prelude::*;
use log::{debug, info, warn};
use super_stt_shared::models::protocol::NotificationEvent;
use super_stt_shared::models::provider::Provider;

impl AppModel {
    pub(in crate::core::app) fn handle_daemon_events(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::DaemonEventsReceived(events) => {
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

    pub(in crate::core::app) fn process_daemon_status_event(
        &mut self,
        event: &NotificationEvent,
    ) -> Option<Task<cosmic::Action<Message>>> {
        info!("Received daemon event: {:?}", event.data);
        let status = event.data.get("status").and_then(|s| s.as_str())?;
        match status {
            "ready" => self.handle_daemon_ready_event(event),
            "device_switch_error" | "error" => self.handle_daemon_device_error_event(event),
            "model_switched" => self.handle_daemon_model_switched_event(event),
            "switching_device" => {
                info!("Received switching_device event: {:?}", event.data);
                // Keep device_state as Switching and wait for "ready" event
                // This event just confirms the switch is in progress
                if !matches!(self.device_state, DeviceState::Switching { .. }) {
                    warn!("Received switching_device event but not in switching state");
                    if let Some(to_device) = event.data.get("to_device").and_then(|d| d.as_str()) {
                        self.set_device_switching(
                            to_device.to_string(),
                            "Switching device...".to_string(),
                        );
                    }
                }
                None
            }
            "loading_model_for_device" => {
                info!("Received loading_model_for_device event: {:?}", event.data);
                if let (Some(target_device), Some(model)) = (
                    event.data.get("target_device").and_then(|d| d.as_str()),
                    event.data.get("model").and_then(|m| m.as_str()),
                ) {
                    let status_message = format!(
                        "Loading {} on {}...",
                        model,
                        if target_device == "cpu" { "CPU" } else { "GPU" }
                    );
                    self.set_device_switching(target_device.to_string(), status_message);
                }
                None
            }
            "settings_changed" => {
                // A setting changed — possibly from another client, or the
                // global Primary Language this very client just set. Re-fetch
                // the language state so a per-model button that follows the
                // global value, and the global card, reflect the new value.
                if event.data.get("setting").and_then(|s| s.as_str()) != Some("language") {
                    return None;
                }
                let mut tasks = vec![self.load_primary_language()];
                if let Some((source, model)) = self.model_language_for.clone() {
                    tasks.push(self.load_model_language(source, model));
                }
                Some(Task::batch(tasks))
            }
            _ => {
                info!("Received unhandled daemon status: {status}");
                None
            }
        }
    }

    pub(in crate::core::app) fn handle_daemon_ready_event(
        &mut self,
        event: &NotificationEvent,
    ) -> Option<Task<cosmic::Action<Message>>> {
        // Handle device readiness
        if let Some(actual_device) = event.data.get("actual_device").and_then(|d| d.as_str()) {
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
        if event
            .data
            .get("model_loaded")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
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
        None
    }

    pub(in crate::core::app) fn handle_daemon_device_error_event(
        &mut self,
        event: &NotificationEvent,
    ) -> Option<Task<cosmic::Action<Message>>> {
        warn!("Received device switch error event: {:?}", event.data);
        // Reset device state from switching to ready
        if matches!(self.device_state, DeviceState::Switching { .. }) {
            info!("Device switch failed, reverting to ready state");
        }
        self.device_state = DeviceState::Ready;
        if let Some(error_msg) = event.data.get("error").and_then(|e| e.as_str()) {
            let error_message = error_msg.to_string();
            // Show error to user
            return Some(Task::perform(async move { error_message }, |msg| {
                cosmic::Action::App(Message::DeviceError(msg))
            }));
        }
        None
    }

    pub(in crate::core::app) fn handle_daemon_model_switched_event(
        &mut self,
        event: &NotificationEvent,
    ) -> Option<Task<cosmic::Action<Message>>> {
        if let Some(model_name) = event.data.get("model_name").and_then(|m| m.as_str()) {
            let model = model_name.to_string();
            let provider = event
                .data
                .get("provider")
                .and_then(|p| p.as_str())
                .and_then(|s| s.parse::<Provider>().ok())
                .unwrap_or_else(|| self.current_provider.clone());
            let source = event
                .data
                .get("source")
                .and_then(|p| p.as_str())
                .map_or_else(|| self.current_source.clone(), str::to_string);
            info!(
                "Received model_switched event: current_model={:?} -> {:?} via {provider} ({source})",
                self.current_model, model
            );
            // A live identity change supersedes any in-flight reconnect
            // snapshot: bump the epoch so a stale get_current_model response
            // can't revert this.
            self.current_model_epoch = self.current_model_epoch.wrapping_add(1);
            self.current_model.clone_from(&model);
            self.current_provider = provider;
            self.current_source.clone_from(&source);
            self.model_operation_state = ModelOperationState::Ready;
            info!("Model state updated to Ready after model_switched event");
            // Mirror CurrentModelLoaded/ModelChanged: fetch the per-model
            // language block so the active-backend card's language button shows
            // the model's resolved language instead of the neutral "Language"
            // label. A client that learns the active model only via this
            // broadcast — e.g. the settings app reconnecting after a daemon
            // restart, where the startup load now emits model_switched — would
            // otherwise leave model_language_for unset.
            return Some(self.load_model_language(source, model));
        }
        None
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
                cosmic::Action::App(Message::DownloadCompleted(model))
            }));
        } else if progress.status == "cancelled" {
            return Some(Task::perform(async move { progress.model_name }, |model| {
                cosmic::Action::App(Message::DownloadCancelled(model))
            }));
        }
        None
    }
}
