// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, DeviceState};
use crate::daemon::client::set_device;
use crate::ui::messages::Message;
use cosmic::prelude::*;
use log::info;
use log::warn;

impl AppModel {
    /// Handle device management messages
    pub(in crate::core::app) fn handle_device_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::DeviceSelected(device) => {
                if device != self.current_device && self.device_state == DeviceState::Ready {
                    self.set_device_switching(device.clone(), "Switching device...".to_string());
                    self.last_device_switch = Some(std::time::Instant::now());

                    info!("Switching to device: {device}");
                    let target_device = device.clone();
                    Task::perform(
                        async move {
                            // Send device switch command and trust the daemon's response
                            match set_device(target_device.clone()).await {
                                Ok(()) => {
                                    // Device switch command succeeded - assume the target device is now active
                                    // We don't verify with get_device to avoid premature requests
                                    info!("Device switch command completed successfully");
                                    Ok(target_device)
                                }
                                Err(e) => Err(e),
                            }
                        },
                        |result| match result {
                            Ok(_device) => {
                                // Don't simulate DeviceInfoLoaded - wait for daemon's "ready" event
                                // to confirm the device switch is actually complete
                                info!(
                                    "Device switch command sent successfully, waiting for daemon confirmation"
                                );
                                cosmic::Action::None
                            }
                            Err(e) => cosmic::Action::App(Message::DeviceError(e)),
                        },
                    )
                } else if matches!(self.device_state, DeviceState::Switching { .. }) {
                    warn!("Device switch already in progress - ignoring");
                    Task::none()
                } else {
                    Task::none()
                }
            }

            Message::DeviceLoaded(device) => {
                self.current_device = device;
                self.device_state = DeviceState::Ready;
                Task::none()
            }

            Message::DeviceInfoLoaded(device, available_devices, gpu_memory) => {
                info!("DeviceInfoLoaded: device={device}, available_devices={available_devices:?}");
                self.current_device.clone_from(&device);
                self.available_devices.clone_from(&available_devices);
                self.gpu_memory = gpu_memory;

                if matches!(self.device_state, DeviceState::Switching { .. }) {
                    info!("Device switch completed to: {device}");
                    self.device_state = DeviceState::Cooldown;
                    // No need to reload models - device switch complete and model state maintained via events
                    Task::none()
                } else {
                    self.device_state = DeviceState::Ready;
                    Task::none()
                }
            }

            Message::DeviceError(err) => {
                self.device_state = DeviceState::Ready;
                self.transcription_text = format!("Device Error: {err}");
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
