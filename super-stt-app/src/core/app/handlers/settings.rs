// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::daemon::client::{
    set_notification_method, set_preview_typing, set_recording_stop_mode, set_write_method,
    test_write_method,
};
use crate::state::device_offers::PP_STAGE;
use crate::ui::messages::{
    Message, NotificationMethodMessage, PostProcessorMessage, PreviewTypingMessage,
    RecordingStopModeMessage, WriteMethodMessage,
};
use cosmic::prelude::*;

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

    /// Handle post-processor messages: stage 2's own block arriving from the
    /// daemon.
    ///
    /// Selecting a backend, staging a model or a device, loading and unloading
    /// all go through [`StageMessage`] — the same handler every stage uses.
    /// What is left here is the state read-back, which stage 1 does not have a
    /// counterpart for: it announces its identity through `ModelMessage`
    /// events instead.
    pub(in crate::core::app) fn handle_post_processor_messages(
        &mut self,
        message: PostProcessorMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            PostProcessorMessage::ReloadRequested => Task::perform(
                crate::daemon::client::get_stage(PP_STAGE),
                Self::reloaded_post_processor,
            ),

            PostProcessorMessage::Loaded(state) => {
                self.post_processor = state;
                self.clear_action_error(crate::state::ErrorScope::PostProcessing);
                // Stage 2 has no completion event of its own: the daemon's
                // `ready` reports stage 1's load, and nothing reports this one,
                // so the progress and loading lines would otherwise sit on the
                // card until the stall watchdog turned them into an error.
                self.finish_post_processor_operation();
                let selection = self.post_processor.selection();
                let source = self.post_processor.source.clone();
                Task::batch([
                    // The card's device chips are the daemon's answer for the
                    // backend now selected, so every selection asks again.
                    source.map_or_else(Task::none, |source| {
                        Self::load_backend_devices(PP_STAGE, source)
                    }),
                    // The model the daemon remembers is the one the card offers
                    // to load, so it is staged like any other pick.
                    selection.map_or_else(Task::none, |(model, source)| {
                        self.stage_selection_if_unstaged(PP_STAGE, &model, &source)
                    }),
                ])
            }

            PostProcessorMessage::Error(err) => {
                log::warn!("Post-processor error: {err}");
                self.set_action_error(
                    crate::state::ErrorScope::PostProcessing,
                    format!("Couldn't save the post-processor: {err}"),
                );
                // The failure ends the operation too, and the banner above is
                // where it is reported.
                self.finish_post_processor_operation();
                Task::none()
            }
        }
    }

    /// End a stage-2 model operation, leaving a stage-1 one alone.
    ///
    /// Stage 2 has no completion event of its own: the daemon's `ready` is
    /// stage 1's, and a post-processor's download and load are reported only by
    /// the `download_progress` ticks that name its model. The write's own
    /// response is the end of it.
    pub(in crate::core::app) fn finish_post_processor_operation(&mut self) {
        self.model_operations.set_ready(PP_STAGE);
    }

    /// Map a `GET /pipeline/2` result into its message.
    fn reloaded_post_processor(
        result: super_stt_shared::daemon::http_client::HttpResult<
            crate::daemon::client::StageState,
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
