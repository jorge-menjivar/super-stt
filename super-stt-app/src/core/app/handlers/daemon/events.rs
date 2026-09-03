// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, DeviceState};
use crate::state::device_offers::{PP_STAGE, STT_STAGE};
use crate::ui::messages::{DaemonMessage, DeviceMessage, DownloadMessage, Message, UpdateMessage};
use cosmic::prelude::*;
use log::{debug, info, warn};
use super_stt_shared::models::protocol::{DaemonStatusEvent, NotificationEvent};

/// How a `SettingsChanged { setting }` SSE event should refresh the Updates
/// page's state, for the two self-update settings (`language` needs
/// `AppModel` state — `model_language_for` — to build its follow-up task, so
/// it's matched separately in the caller and never reaches this function).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::core::app) enum SelfUpdateSettingRoute {
    /// Reload the toggler value and re-fetch the cached status. Cheap, and
    /// correct here because `update_check_enabled` doesn't change what the
    /// next check would compute.
    RefreshCachedStatus,
    /// Force a real re-check rather than trust the cached status.
    ForceRecheck,
}

/// `update_beta_optin` MUST route through a real re-check (`ForceRecheck`,
/// which the caller sends via `CheckNow`) rather than a cached status
/// refetch: `beta_optin_effective` is computed server-side at check time,
/// not stored, so a plain refetch would show the STALE value until whatever
/// triggered this event's own check lands — a regression a previous review
/// round fixed. `update_check_enabled` has no such effective-at-check-time
/// field, so the cheaper cached refresh is correct for it. Any other
/// setting name (including `language`) isn't a self-update setting at all.
pub(in crate::core::app) fn self_update_setting_route(
    setting: &str,
) -> Option<SelfUpdateSettingRoute> {
    match setting {
        "update_check_enabled" => Some(SelfUpdateSettingRoute::RefreshCachedStatus),
        "update_beta_optin" => Some(SelfUpdateSettingRoute::ForceRecheck),
        _ => None,
    }
}

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
            // Stage 2 reports its own lifecycle on this topic, so every arm
            // that acts on a stage's model checks whose it is first: reading a
            // post-processor's load as the transcription model's would clear
            // the wrong card and report the wrong device.
            DaemonStatusEvent::Ready { stage, .. } if stage == PP_STAGE => {
                self.finish_post_processor_operation();
                // The daemon is authoritative about what stage 2 is running,
                // and this is the only notice of a load it started itself (at
                // startup, say) or one another client asked for.
                Some(crate::core::app::handlers::tasks::load_post_processor())
            }
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
            // Stage 2's identity change needs no separate handling: the
            // `ready` that follows it re-reads the post-processor block, which
            // is where the app keeps that stage's identity.
            DaemonStatusEvent::ModelSwitched { stage, .. } if stage == PP_STAGE => None,
            DaemonStatusEvent::ModelSwitched {
                model_name, source, ..
            } => Some(self.handle_daemon_model_switched(&model_name, &source)),
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
                ..
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
                // A setting changed — possibly from another client, or this
                // very client's own change coming back over SSE.
                match setting.as_str() {
                    "language" => {
                        // Re-fetch the language state so a per-model button that
                        // follows the global value, and the global card, reflect
                        // the new value.
                        // Every pair answered for, not just one: both stages
                        // show a language control, and a block that follows the
                        // global value goes stale on either card.
                        let pairs: Vec<(String, String)> =
                            self.language.model_languages.pairs().collect();
                        let mut tasks = vec![self.load_primary_language()];
                        tasks.extend(
                            pairs
                                .into_iter()
                                .map(|(source, model)| self.load_model_language(source, model)),
                        );
                        Some(Task::batch(tasks))
                    }
                    // Another client (or this one) changed the post-processor.
                    // Re-read it so the section shows the current selection and
                    // its live `loaded` state.
                    "post_processor" => {
                        Some(crate::core::app::handlers::tasks::load_post_processor())
                    }
                    // Both self-update settings need something beyond a plain
                    // `SettingsChanged` no-op, but *what* they need differs —
                    // see `self_update_setting_route`'s doc comment for why
                    // `update_beta_optin` can't just reuse the cached-refresh
                    // path `update_check_enabled` uses.
                    other => match self_update_setting_route(other) {
                        Some(SelfUpdateSettingRoute::RefreshCachedStatus) => {
                            Some(Task::batch(vec![
                                crate::core::app::handlers::tasks::load_update_check_enabled(),
                                crate::core::app::handlers::tasks::refresh_update_status(),
                            ]))
                        }
                        Some(SelfUpdateSettingRoute::ForceRecheck) => {
                            // `CheckNow`'s `if self.update.checking { return }`
                            // guard makes this a no-op for the app's own
                            // self-triggered change (its `BetaOptinToggled`
                            // already chains `CheckNow`), while a genuine
                            // cross-app change still gets a correct fresh
                            // recompute.
                            Some(self.handle_update_messages(UpdateMessage::CheckNow))
                        }
                        None => None,
                    },
                }
            }
            // Stage 2's load-start is the card's only cue for a load with
            // nothing to download, and for one this app did not ask for.
            DaemonStatusEvent::LoadingModel { new_model, stage } if stage == PP_STAGE => {
                self.set_model_loading(new_model, "Loading model...".to_string(), PP_STAGE);
                None
            }
            // Stage 1's is still ignored: its own Load click already set the
            // card loading, and a load from elsewhere is reported by the
            // `model_switched` / `ready` pair that closes it.
            DaemonStatusEvent::LoadingModel { .. }
            | DaemonStatusEvent::ActiveBackendChanged { .. } => None,
            // The daemon completed a periodic self-update check and found a
            // newer release. Re-fetch the status so the header badge and the
            // Updates page pick it up without waiting for the user to open
            // that page.
            DaemonStatusEvent::UpdateAvailable { latest_version } => {
                log::info!("Daemon reports update available: {latest_version}");
                Some(self.handle_update_messages(UpdateMessage::AvailableEventReceived))
            }
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
            // Stage 2's `ready` is intercepted before this, so this one is
            // stage 1's.
            info!(
                "Stage 1 model state before ready event: {:?}",
                self.model_operations.get(STT_STAGE)
            );
            self.model_operations.set_ready(STT_STAGE);
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
        source: &str,
    ) -> Task<cosmic::Action<Message>> {
        let model = model_name.to_string();
        let source = source.to_string();
        info!(
            "Received model_switched event: current_model={:?} -> {:?} from source ({source})",
            self.current_model, model
        );
        // A live identity change supersedes any in-flight reconnect snapshot: bump
        // the epoch so a stale get_current_model response can't revert this.
        self.current_model_epoch = self.current_model_epoch.wrapping_add(1);
        self.current_model.clone_from(&model);
        self.current_source.clone_from(&source);
        self.model_operations.set_ready(STT_STAGE);
        info!("Stage 1 model state updated to Ready after model_switched event");
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
            let (model, stage) = (progress.model_name, progress.stage);
            return Some(Task::perform(
                async move { (model, stage) },
                |(model, stage)| {
                    cosmic::Action::App(Message::Download(DownloadMessage::DownloadCancelled {
                        model,
                        stage,
                    }))
                },
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{SelfUpdateSettingRoute, self_update_setting_route};

    /// The regression this pins: `update_beta_optin` must force a real
    /// re-check, never the cheaper cached-status path — `beta_optin_effective`
    /// is computed server-side at check time and isn't part of the cached
    /// status a plain refetch would return.
    #[test]
    fn update_beta_optin_forces_a_recheck() {
        assert_eq!(
            self_update_setting_route("update_beta_optin"),
            Some(SelfUpdateSettingRoute::ForceRecheck)
        );
    }

    #[test]
    fn update_check_enabled_uses_the_cached_refresh() {
        assert_eq!(
            self_update_setting_route("update_check_enabled"),
            Some(SelfUpdateSettingRoute::RefreshCachedStatus)
        );
    }

    #[test]
    fn other_settings_including_language_are_not_self_update_settings() {
        assert_eq!(self_update_setting_route("language"), None);
        assert_eq!(self_update_setting_route("something_else"), None);
    }
}
