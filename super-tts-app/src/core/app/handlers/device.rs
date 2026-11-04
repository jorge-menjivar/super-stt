// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, DeviceState, ModelOperationState};
use crate::daemon::client::v1::pipeline::device::ModelDevice;
use crate::daemon::client::{get_model_device, get_stage_model, list_model_devices};
use crate::ui::messages::{DeviceMessage, Message};
use cosmic::prelude::*;
use log::info;
use super_tts_shared::models::protocol::SYNTHESIS_STAGE;

impl AppModel {
    /// Handle device management messages
    pub(in crate::core::app) fn handle_device_messages(
        &mut self,
        message: DeviceMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            DeviceMessage::StageDevicesLoaded(devices) => {
                info!("Stage device list loaded: {devices:?}");
                self.stage_devices = devices;
                Task::none()
            }

            DeviceMessage::ModelDeviceLoaded {
                source,
                model,
                device,
            } => {
                info!(
                    "Device for {source}/{model}: preference={}, resolved={:?}, offered={:?}",
                    device.device, device.resolved_accel, device.available_devices
                );
                self.seed_staged_device(&model, &device);
                self.model_device = Some(device);
                self.model_device_for = Some((source, model));
                Task::none()
            }

            DeviceMessage::ModelDevicesListed {
                source,
                model,
                devices,
            } => {
                // Narrow on purpose: only the offered set is replaced, so a
                // preference the user set stays exactly as they set it. Applied
                // only to the block it describes — the pair may have moved on
                // while the read was in flight.
                if self.model_device_for.as_ref() == Some(&(source, model))
                    && let Some(held) = self.model_device.as_mut()
                {
                    held.available_devices = devices;
                }
                Task::none()
            }

            DeviceMessage::RunningDeviceLoaded(device) => {
                // Empty rather than a placeholder: the card's "Active:" line
                // simply drops the device suffix when there is no local
                // accelerator worth naming, which is what an online model and a
                // model that is not up both come back as.
                self.current_device = device.unwrap_or_default();
                Task::none()
            }

            DeviceMessage::DeviceError(err) => {
                // A device change is a Models-page operation, so surface the
                // failure on that page's card banner rather than hijacking the
                // Speech page's test panel (Tier 3 #11).
                self.device_state = DeviceState::Ready;
                self.model_operation_state = ModelOperationState::Error {
                    message: format!("Device error: {err}"),
                };
                Task::none()
            }
        }
    }

    /// Read one model's device: the preference it would load with, what that
    /// resolved to, and what this host can offer it.
    ///
    /// Issued whenever a model becomes the card's subject — staged from the
    /// dropdown, restored from the stage view, or handed back by an unload. The
    /// answer is per model and cannot be derived here: two backends may serve
    /// the same model name, and only the daemon knows which accelerators the
    /// installed asset actually shipped with on this host.
    #[allow(clippy::unused_self)]
    pub(in crate::core::app) fn load_model_device(
        &self,
        source: &str,
        model: String,
    ) -> Task<cosmic::Action<Message>> {
        let src = source.to_string();
        let mdl = model.clone();
        Task::perform(
            get_model_device(SYNTHESIS_STAGE, model),
            move |result| match result {
                Ok(device) => {
                    cosmic::Action::App(Message::Device(DeviceMessage::ModelDeviceLoaded {
                        source: src.clone(),
                        model: mdl.clone(),
                        device,
                    }))
                }
                Err(e) => {
                    cosmic::Action::App(Message::Device(DeviceMessage::DeviceError(e.to_string())))
                }
            },
        )
    }

    /// Re-read only which devices one model may be loaded onto.
    ///
    /// For the case the whole block would get wrong: a backend update can
    /// replace a CPU-only asset with an accelerated one — or the reverse — and
    /// that changes what the picker may offer without changing what the model
    /// is set to. Re-reading `get_model_device` here would work too, and would
    /// also overwrite a preference the user staged moments earlier with the
    /// stored one, for a change that never touched it.
    #[allow(clippy::unused_self)]
    pub(in crate::core::app) fn refresh_model_devices(
        &self,
        source: &str,
        model: String,
    ) -> Task<cosmic::Action<Message>> {
        let src = source.to_string();
        let mdl = model.clone();
        Task::perform(
            list_model_devices(SYNTHESIS_STAGE, model),
            move |result| match result {
                Ok(devices) => {
                    cosmic::Action::App(Message::Device(DeviceMessage::ModelDevicesListed {
                        source: src.clone(),
                        model: mdl.clone(),
                        devices,
                    }))
                }
                Err(e) => {
                    cosmic::Action::App(Message::Device(DeviceMessage::DeviceError(e.to_string())))
                }
            },
        )
    }

    /// Read back the accelerator the stage's model actually came up on.
    ///
    /// Issued once a load reports success, because until then there is nothing
    /// to report: a `gpu` preference can fall back to the CPU, and the card
    /// must never name an accelerator no load confirmed. The `ready` broadcast
    /// carries the same fact and usually arrives first, but it is a broadcast —
    /// it is not ordered against the switch that caused it, and a client that
    /// depended on it alone would show a device-less "Active:" line whenever it
    /// arrived early or was missed across a reconnect.
    ///
    /// A failure is swallowed to `None`: the suffix is cosmetic, and an error
    /// banner over a model that loaded perfectly well would be the worse lie.
    #[allow(clippy::unused_self)]
    pub(in crate::core::app) fn load_running_device(&self) -> Task<cosmic::Action<Message>> {
        Task::perform(get_stage_model(SYNTHESIS_STAGE), |result| {
            let device = result
                .ok()
                .and_then(|slot| slot.running_device().map(ToString::to_string));
            cosmic::Action::App(Message::Device(DeviceMessage::RunningDeviceLoaded(device)))
        })
    }

    /// Point the card's device picker at this model's answer, unless something
    /// is already staged.
    ///
    /// Two guards, each for a late reply. The first: an answer for a model the
    /// user has since moved off must not seed the picker for whatever is staged
    /// now. The second: staging a model clears the device, so any value present
    /// when this lands is either this same answer arriving twice or a pick the
    /// user made from the stage-wide list while it was in flight — and
    /// overwriting a device the user just chose, a beat after they chose it, is
    /// the kind of thing that looks like the dropdown fighting back.
    fn seed_staged_device(&mut self, model: &str, reported: &ModelDevice) {
        if self.models_page.staged_model.as_deref() != Some(model) {
            return;
        }
        if self.models_page.staged_device.is_some() {
            return;
        }
        self.models_page.staged_device = staged_device_for(reported);
    }
}

/// The device a freshly staged model starts on: the preference the daemon holds
/// for it when this host can still honor it, otherwise the first device this
/// host can offer, and `None` when it can offer none.
///
/// The stored preference comes first because it is the device the model *would*
/// load with — showing anything else in the picker would misreport what Load is
/// about to do. But a stored preference is not a promise: a `gpu` recorded on a
/// machine that has since lost its card, or against a backend whose reinstall
/// resolved to a CPU-only asset, is no longer offerable, and staging it would
/// leave the dropdown rendering with nothing selected while Load sent a device
/// the daemon then refuses.
///
/// `None` is what the Load button reads as "not ready": a model that can run on
/// no device here — including the online sentinel, which reports `none` and an
/// empty list — has nothing to stage, and the card says so instead of offering
/// a button that cannot work.
fn staged_device_for(reported: &ModelDevice) -> Option<String> {
    if reported.available_devices.contains(&reported.device) {
        return Some(reported.device.clone());
    }
    reported.available_devices.first().cloned()
}

#[cfg(test)]
mod staged_device_tests {
    use super::{ModelDevice, staged_device_for};

    fn reported(device: &str, available: &[&str]) -> ModelDevice {
        ModelDevice {
            device: device.to_string(),
            resolved_accel: None,
            available_devices: available.iter().map(|d| (*d).to_string()).collect(),
        }
    }

    /// The ordinary case: the picker opens on the device the model would
    /// actually load with, not on whatever happens to be first in the list.
    #[test]
    fn an_offerable_preference_is_what_gets_staged() {
        assert_eq!(
            staged_device_for(&reported("gpu", &["cpu", "gpu"])),
            Some("gpu".to_string()),
        );
        assert_eq!(
            staged_device_for(&reported("cpu", &["cpu", "gpu"])),
            Some("cpu".to_string()),
        );
    }

    /// The regression this guards: a `gpu` preference recorded before the
    /// backend was reinstalled onto a CPU-only asset is no longer offerable.
    /// Staging it anyway renders the dropdown with nothing selected — the value
    /// is not in the list it draws from — while Load sends a device the daemon
    /// refuses. The staged device must always be one the picker offers.
    #[test]
    fn an_unofferable_preference_falls_back_to_what_is_offered() {
        let staged = staged_device_for(&reported("gpu", &["cpu"]));
        assert_eq!(staged, Some("cpu".to_string()));
        assert!(
            reported("gpu", &["cpu"])
                .available_devices
                .contains(&staged.expect("staged")),
            "the staged device must be one the picker offers"
        );
    }

    /// Nothing offered, nothing staged — the online sentinel (`none`, empty
    /// list) and a local model this install can run nowhere both land here, and
    /// both leave the Load button with no device to commit.
    #[test]
    fn nothing_offered_stages_nothing() {
        assert_eq!(staged_device_for(&reported("none", &[])), None);
        assert_eq!(staged_device_for(&reported("gpu", &[])), None);
    }
}
