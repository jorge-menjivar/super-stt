// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::{
    clear_post_processor, clear_post_processor_backend, get_model_device, set_model_device,
    set_notification_method, set_post_processor, set_post_processor_backend, set_preview_typing,
    set_recording_stop_mode, set_write_method, test_write_method,
};
use crate::ui::messages::{
    Message, NotificationMethodMessage, PostProcessorMessage, PreviewTypingMessage,
    RecordingStopModeMessage, WriteMethodMessage,
};
use cosmic::prelude::*;

/// Post-processing is stage 2 of the pipeline — the stage a staged
/// post-processor's device is addressed through.
const PP_STAGE: u32 = 2;

impl AppModel {
    /// Handle preview typing messages
    pub(in crate::core::app) fn handle_preview_typing_messages(
        &mut self,
        message: PreviewTypingMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Confirm-then-apply: the local value is set only when the daemon
            // acks the save (via `PreviewTypingSettingLoaded`). A failed POST
            // therefore leaves the toggle on its old, correct value instead of
            // stranding an un-rolled-back optimistic one (Tier 1 #15).
            PreviewTypingMessage::Toggled(enabled) => {
                Task::perform(set_preview_typing(enabled), move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::PreviewTyping(
                        PreviewTypingMessage::SettingLoaded(enabled),
                    )),
                    Err(e) => cosmic::Action::App(Message::PreviewTyping(
                        PreviewTypingMessage::Error(e.to_string()),
                    )),
                })
            }

            PreviewTypingMessage::SettingLoaded(enabled) => {
                self.preview_typing_enabled = enabled;
                self.clear_action_error(crate::state::ErrorScope::Recording);
                Task::none()
            }

            PreviewTypingMessage::Error(err) => {
                // The toggle already reflects the daemon's last-known value (we
                // never applied optimistically), so there's nothing to roll back
                // — surface the failure on the Recording page's banner instead of
                // only logging it (Tier 3 #11).
                log::warn!("Preview typing error: {err}");
                self.set_action_error(
                    crate::state::ErrorScope::Recording,
                    format!("Couldn't save preview typing: {err}"),
                );
                Task::none()
            }
        }
    }

    /// Handle post-processor messages.
    ///
    /// Confirm-then-apply throughout, like every other setting here: the local
    /// state is replaced only by what the daemon reports back, so a refused or
    /// failed save leaves the card showing the truth rather than an optimistic
    /// value that never took.
    pub(in crate::core::app) fn handle_post_processor_messages(
        &mut self,
        message: PostProcessorMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Selecting a backend is the "chosen, not running" state, the same
            // one `SelectBackend` gives transcription — and it goes to the same
            // shape of endpoint, so nothing loads until a model is enabled.
            PostProcessorMessage::SelectBackend(source) => {
                self.staged_post_processor = None;
                self.staged_post_processor_device = None;
                // The choice came from the picker sheet; dismiss it now that it
                // has been made, exactly as `SelectBackend` does.
                self.core.window.show_context = false;
                Task::perform(
                    set_post_processor_backend(source),
                    Self::reload_post_processor,
                )
            }

            PostProcessorMessage::Deselect => {
                self.staged_post_processor = None;
                self.staged_post_processor_device = None;
                Task::perform(clear_post_processor_backend(), Self::reload_post_processor)
            }

            // Local only, exactly like staging a transcription model: picking
            // from the dropdown must not start running a model.
            PostProcessorMessage::Staged(index) => self.stage_post_processor(index),

            PostProcessorMessage::StagedDevice(device) => {
                self.staged_post_processor_device = Some(device);
                Task::none()
            }

            PostProcessorMessage::StagedDeviceLoaded { model, device } => {
                // The answer is for the model staged when it was asked; a pick
                // made since wins, and so does a pick the dropdown cannot show.
                if let Some((staged, source)) = &self.staged_post_processor
                    && *staged == model
                    && let Some(backend) = self.backends.iter().find(|b| &b.source == source)
                    && crate::ui::views::models::offered_devices(backend, &model)
                        .contains(&device.device)
                {
                    self.staged_post_processor_device = Some(device.device);
                }
                Task::none()
            }

            PostProcessorMessage::Enable => self.enable_post_processor(),

            // Off, but the selection is kept: the user is turning the feature
            // off, not forgetting which model they picked. That is exactly what
            // `DELETE /post_processor` means, so the call carries no state to
            // echo back and cannot accidentally clear the backend.
            PostProcessorMessage::Disable => {
                Task::perform(clear_post_processor(), Self::reload_post_processor)
            }

            PostProcessorMessage::ReloadRequested => Task::perform(
                crate::daemon::client::get_post_processor(),
                Self::reloaded_post_processor,
            ),

            PostProcessorMessage::Loaded(state) => {
                // The daemon is authoritative, so a staged pick that has landed
                // is no longer pending. Clearing it also keeps the dropdown
                // showing the daemon's selection after a refused save.
                if self.staged_post_processor == state.selection() {
                    self.staged_post_processor = None;
                    self.staged_post_processor_device = None;
                }
                self.post_processor = state;
                self.clear_action_error(crate::state::ErrorScope::PostProcessing);
                Task::none()
            }

            PostProcessorMessage::Error(err) => {
                log::warn!("Post-processor error: {err}");
                self.set_action_error(
                    crate::state::ErrorScope::PostProcessing,
                    format!("Couldn't save the post-processor: {err}"),
                );
                Task::none()
            }
        }
    }

    /// Stage the post-processor at `index` in the selected backend's list:
    /// seed its device from what this install offers, then ask the daemon for
    /// the model's own — the same two steps staging a transcription model
    /// takes.
    fn stage_post_processor(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        let Some(backend) = self
            .post_processor
            .source
            .as_deref()
            .and_then(|source| self.backends.iter().find(|b| b.source == source))
        else {
            return Task::none();
        };
        let Some(model) = crate::ui::views::models::post_processor_models(backend)
            .into_iter()
            .nth(index)
        else {
            return Task::none();
        };
        let device = crate::ui::views::models::offered_devices(backend, &model)
            .first()
            .cloned();
        self.staged_post_processor = Some((model.clone(), backend.source.clone()));
        self.staged_post_processor_device.clone_from(&device);
        self.clear_action_error(crate::state::ErrorScope::PostProcessing);
        // Nothing to ask for an online model, which stages no device at all.
        if device.is_none() {
            return Task::none();
        }
        Task::perform(get_model_device(PP_STAGE, model.clone()), move |result| {
            match result {
                Ok(device) => cosmic::Action::App(Message::PostProcessor(
                    PostProcessorMessage::StagedDeviceLoaded {
                        model: model.clone(),
                        device,
                    },
                )),
                // The local default is already staged; a failed read only
                // means the model's own choice is not reflected.
                Err(e) => {
                    log::warn!("Could not read the device for {model}: {e}");
                    cosmic::Action::None
                }
            }
        })
    }

    /// Commit the staged pick — or re-enable the daemon's own selection —
    /// setting the staged model's device first when one is staged.
    fn enable_post_processor(&mut self) -> Task<cosmic::Action<Message>> {
        // The staged pick wins; otherwise re-enable whatever model is already
        // selected. With neither — a backend chosen but no model picked —
        // there is nothing to run.
        let Some((model, source)) = self
            .staged_post_processor
            .clone()
            .or_else(|| self.post_processor.selection())
        else {
            self.set_action_error(
                crate::state::ErrorScope::PostProcessing,
                "Choose a model first.".to_string(),
            );
            return Task::none();
        };
        // A staged device belongs to the staged model; re-enabling the
        // daemon's own selection keeps whatever device it has.
        let device = self
            .staged_post_processor
            .as_ref()
            .and_then(|_| self.staged_post_processor_device.clone());
        Task::perform(
            async move {
                // Set before the load so the load picks it up; for a model
                // that is not loaded the daemon only records it.
                if let Some(device) = device {
                    set_model_device(PP_STAGE, model.clone(), device).await?;
                }
                set_post_processor(model, Some(source)).await
            },
            Self::reload_post_processor,
        )
    }

    /// Map a set result into the follow-up that re-reads the daemon's own
    /// state. The write's response carries no payload, and the daemon may have
    /// adjusted things (a selection that would not load, say), so the card
    /// renders what the daemon reports rather than what was sent.
    fn reload_post_processor(
        result: super_stt_shared::daemon::http_client::HttpResult<()>,
    ) -> cosmic::Action<Message> {
        match result {
            Ok(()) => cosmic::Action::App(Message::PostProcessor(
                PostProcessorMessage::ReloadRequested,
            )),
            Err(e) => cosmic::Action::App(Message::PostProcessor(PostProcessorMessage::Error(
                e.to_string(),
            ))),
        }
    }

    /// Map a `GET /post_processor` result into its message.
    fn reloaded_post_processor(
        result: super_stt_shared::daemon::http_client::HttpResult<
            crate::daemon::client::PostProcessorState,
        >,
    ) -> cosmic::Action<Message> {
        match result {
            Ok(state) => {
                cosmic::Action::App(Message::PostProcessor(PostProcessorMessage::Loaded(state)))
            }
            Err(e) => cosmic::Action::App(Message::PostProcessor(PostProcessorMessage::Error(
                e.to_string(),
            ))),
        }
    }

    /// Handle recording stop mode messages
    pub(in crate::core::app) fn handle_recording_stop_mode_messages(
        &mut self,
        message: RecordingStopModeMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Confirm-then-apply (see `PreviewTypingToggled`): apply only on the
            // daemon ack so a failed save doesn't strand an optimistic value.
            RecordingStopModeMessage::Changed(mode) => {
                let mode_str = mode.to_string();
                Task::perform(
                    set_recording_stop_mode(mode_str),
                    move |result| match result {
                        Ok(()) => cosmic::Action::App(Message::RecordingStopMode(
                            RecordingStopModeMessage::Loaded(mode),
                        )),
                        Err(e) => cosmic::Action::App(Message::RecordingStopMode(
                            RecordingStopModeMessage::Error(e.to_string()),
                        )),
                    },
                )
            }

            RecordingStopModeMessage::Loaded(mode) => {
                self.recording_stop_mode = mode;
                self.clear_action_error(crate::state::ErrorScope::Recording);
                Task::none()
            }

            RecordingStopModeMessage::Error(err) => {
                log::warn!("Recording stop mode error: {err}");
                self.set_action_error(
                    crate::state::ErrorScope::Recording,
                    format!("Couldn't save recording stop mode: {err}"),
                );
                Task::none()
            }
        }
    }

    /// Handle write method messages
    pub(in crate::core::app) fn handle_write_method_messages(
        &mut self,
        message: WriteMethodMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Confirm-then-apply (see `PreviewTypingToggled`): apply only on the
            // daemon ack so a failed save doesn't strand an optimistic value.
            WriteMethodMessage::Changed(method) => {
                let method_str = method.to_string();
                Task::perform(set_write_method(method_str), move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::WriteMethod(
                        WriteMethodMessage::Loaded(method),
                    )),
                    Err(e) => cosmic::Action::App(Message::WriteMethod(WriteMethodMessage::Error(
                        format!("couldn't save: {e}"),
                    ))),
                })
            }

            WriteMethodMessage::Loaded(method) => {
                self.write_method = method;
                // The stored resolution belonged to the previous method.
                self.resolved_write_method = None;
                self.clear_action_error(crate::state::ErrorScope::InputSimulation);
                Task::none()
            }

            // Focus the test field *before* asking the daemon to type: it types
            // into whatever window holds focus, and pressing the button leaves
            // focus on the button. `chain` orders the two, where `batch` would
            // race the focus against the round-trip.
            WriteMethodMessage::Test => {
                self.write_method_test_text.clear();
                self.clear_action_error(crate::state::ErrorScope::InputSimulation);
                cosmic::widget::text_input::focus(
                    crate::ui::views::input_simulation::test_field_id(),
                )
                .chain(write_method_test_task())
            }

            // The delayed test deliberately does *not* focus the field: the
            // user is switching to another window, and the apps that silently
            // drop simulated keys are exactly the ones this page cannot host.
            WriteMethodMessage::TestDelayed => {
                self.write_method_test_text.clear();
                self.clear_action_error(crate::state::ErrorScope::InputSimulation);
                self.write_method_test_countdown = Some(TEST_COUNTDOWN_SECS);
                countdown_tick_task()
            }

            WriteMethodMessage::TestTick => {
                match advance_countdown(self.write_method_test_countdown) {
                    Tick::Idle => Task::none(),
                    Tick::Continue(next) => {
                        self.write_method_test_countdown = Some(next);
                        countdown_tick_task()
                    }
                    Tick::Fire => {
                        self.write_method_test_countdown = None;
                        write_method_test_task()
                    }
                }
            }

            WriteMethodMessage::TestCancel => {
                self.write_method_test_countdown = None;
                Task::none()
            }

            WriteMethodMessage::Tested(resolved) => {
                self.resolved_write_method = resolved;
                self.write_method_test_countdown = None;
                Task::none()
            }

            WriteMethodMessage::TestInput(text) => {
                self.write_method_test_text = text;
                Task::none()
            }

            WriteMethodMessage::Error(err) => {
                log::warn!("Write method error: {err}");
                self.write_method_test_countdown = None;
                self.set_action_error(
                    crate::state::ErrorScope::InputSimulation,
                    format!("Write method: {err}"),
                );
                Task::none()
            }
        }
    }

    /// Handle notification method messages
    pub(in crate::core::app) fn handle_notification_method_messages(
        &mut self,
        message: NotificationMethodMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // Confirm-then-apply, as with the write method: apply only on the
            // daemon ack so a failed save doesn't strand an optimistic value.
            NotificationMethodMessage::Changed(method) => {
                let method_str = method.to_string();
                Task::perform(
                    set_notification_method(method_str),
                    move |result| match result {
                        Ok(()) => cosmic::Action::App(Message::NotificationMethod(
                            NotificationMethodMessage::Loaded(method),
                        )),
                        Err(e) => cosmic::Action::App(Message::NotificationMethod(
                            NotificationMethodMessage::Error(e.to_string()),
                        )),
                    },
                )
            }

            NotificationMethodMessage::Loaded(method) => {
                self.notification_method = method;
                self.clear_action_error(crate::state::ErrorScope::Recording);
                Task::none()
            }

            NotificationMethodMessage::Error(err) => {
                log::warn!("Notification method error: {err}");
                self.set_action_error(
                    crate::state::ErrorScope::Recording,
                    format!("Couldn't save notification method: {err}"),
                );
                Task::none()
            }
        }
    }
}

/// Seconds a delayed write-method test waits before typing — long enough to
/// alt-tab into the target window, short enough that the user doesn't wonder
/// whether the button worked.
const TEST_COUNTDOWN_SECS: u8 = 3;

/// Ask the daemon to type the test string, mapping the outcome onto the
/// write-method messages. Shared by the immediate and delayed tests, which
/// differ only in what happens *before* this runs.
fn write_method_test_task() -> Task<cosmic::Action<Message>> {
    Task::perform(test_write_method(), |result| match result {
        // `None` is a daemon that typed but named no backend this build
        // knows: the test still passed, so report it and leave the backend
        // readout empty rather than guessing.
        Ok(resolved) => {
            cosmic::Action::App(Message::WriteMethod(WriteMethodMessage::Tested(resolved)))
        }
        Err(e) => cosmic::Action::App(Message::WriteMethod(WriteMethodMessage::Error(format!(
            "test failed: {e}"
        )))),
    })
}

/// What a countdown tick should do.
#[derive(Debug, PartialEq, Eq)]
enum Tick {
    /// No countdown is running — the tick belongs to one already cancelled.
    Idle,
    /// Keep counting, with this many seconds left.
    Continue(u8),
    /// Time is up: type now.
    Fire,
}

/// Advance a countdown by one second.
///
/// `None` in means `Idle` out, and that is the whole point: cancelling clears
/// the countdown but cannot unschedule the tick already in flight, so without
/// this the stale tick would restart the countdown the user just stopped —
/// and then type into their window.
fn advance_countdown(remaining: Option<u8>) -> Tick {
    match remaining {
        None => Tick::Idle,
        // `saturating_sub` also covers a `Some(0)` that no code path should
        // produce: firing is the safe reading of "no time left".
        Some(secs) => match secs.saturating_sub(1) {
            0 => Tick::Fire,
            next => Tick::Continue(next),
        },
    }
}

/// One second of countdown. Re-armed per tick rather than run as a
/// subscription so the timer exists only while a countdown does.
fn countdown_tick_task() -> Task<cosmic::Action<Message>> {
    Task::perform(
        async {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        },
        |()| cosmic::Action::App(Message::WriteMethod(WriteMethodMessage::TestTick)),
    )
}

#[cfg(test)]
mod tests {
    use super::{TEST_COUNTDOWN_SECS, Tick, advance_countdown};

    /// The full run from button press to typing: one tick per second, firing
    /// on the last. Getting this wrong types either a second early or never.
    #[test]
    fn counts_down_once_per_second_then_fires() {
        let mut remaining = Some(TEST_COUNTDOWN_SECS);
        let mut ticks = 0;
        loop {
            match advance_countdown(remaining) {
                Tick::Continue(next) => {
                    remaining = Some(next);
                    ticks += 1;
                }
                Tick::Fire => {
                    ticks += 1;
                    break;
                }
                Tick::Idle => panic!("a running countdown must not report idle"),
            }
        }
        assert_eq!(
            ticks, TEST_COUNTDOWN_SECS,
            "one tick per displayed second, no more"
        );
    }

    /// A cancel clears the countdown but cannot unschedule the tick already in
    /// flight. That tick must do nothing — otherwise the cancelled test still
    /// types into whatever window the user moved to.
    #[test]
    fn a_tick_after_cancel_does_nothing() {
        assert_eq!(advance_countdown(None), Tick::Idle);
    }

    /// Defensive: no path sets zero, but "no time left" can only mean fire.
    #[test]
    fn zero_fires_rather_than_underflowing() {
        assert_eq!(advance_countdown(Some(0)), Tick::Fire);
        assert_eq!(advance_countdown(Some(1)), Tick::Fire);
    }
}
