// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, DeviceState};
use crate::ui::messages::Message;
use cosmic::prelude::*;
use log::info;

impl AppModel {
    /// Handle device management messages
    pub(in crate::core::app) fn handle_device_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::DeviceInfoLoaded(device, available_devices) => {
                info!("DeviceInfoLoaded: device={device}, available_devices={available_devices:?}");
                self.current_device.clone_from(&device);
                self.available_devices.clone_from(&available_devices);

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
