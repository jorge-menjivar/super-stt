// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, DeviceState, ModelOperationState};
use crate::state::device_offers::STT_STAGE;
use crate::ui::messages::{DeviceMessage, Message};
use cosmic::prelude::*;
use log::info;

impl AppModel {
    /// Handle device management messages
    pub(in crate::core::app) fn handle_device_messages(
        &mut self,
        message: DeviceMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            DeviceMessage::DeviceInfoLoaded(device) => {
                info!("DeviceInfoLoaded: device={device:?}");
                self.current_device = device.unwrap_or_default();

                if matches!(self.device_state, DeviceState::Switching { .. }) {
                    info!("Device switch completed to: {}", self.current_device);
                    self.device_state = DeviceState::Cooldown;
                    // No need to reload models - device switch complete and model state maintained via events
                    Task::none()
                } else {
                    self.device_state = DeviceState::Ready;
                    Task::none()
                }
            }

            DeviceMessage::DeviceError(err) => {
                // A device switch is a Models-page operation, so surface the
                // failure on that page's card banner rather than hijacking the
                // Recording page's transcription box (Tier 3 #11).
                self.device_state = DeviceState::Ready;
                // Device switching is the transcription card's control, so its
                // failure belongs on that card.
                self.model_operations.set(
                    STT_STAGE,
                    ModelOperationState::Error {
                        message: format!("Device error: {err}"),
                    },
                );
                Task::none()
            }
        }
    }
}
