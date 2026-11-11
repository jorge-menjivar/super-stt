// SPDX-License-Identifier: GPL-3.0-only
//! Putting speech to text in the state a scene needs, on the running daemon.
//!
//! The Models page draws a different card for each state the transcription
//! stage can be in, and a shot of one needs the daemon in it. So before its
//! shot a scene reads the stage, and asks for one change at a time until the
//! stage is as the scene needs it: the same messages the card's own buttons
//! send, so the app follows each change as it does a click. `just screenshots`
//! reads the state first and puts it back at the end.
//!
//! What to select and load comes from the environment:
//! `SUPER_VOICE_APP_SCENE_STT_SOURCE`, `_MODEL` and `_DEVICE`.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use cosmic::prelude::*;

use super::{AppModel, Scene};
use crate::daemon::client::{StageState, get_stage_view};
use crate::state::Stage;
use crate::ui::messages::{Message, ShellMessage, StageMessage};

/// How long the daemon gets to take a change before the next is asked for.
const TAKE: Duration = Duration::from_secs(1);

/// How long a scene keeps at it. A load can mean preparing kernels.
const LIMIT: Duration = Duration::from_mins(4);

/// How many times one change is asked for before the scene gives up on it.
const ASKS: u32 = 3;

/// What speech to text is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stt {
    /// No backend selected.
    Empty,
    /// A backend selected, and no model picked.
    Selected,
    /// A backend and its model picked, and the model not running.
    Unloaded,
    /// The model running.
    Running,
}

impl Stt {
    const fn name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Selected => "selected",
            Self::Unloaded => "unloaded",
            Self::Running => "running",
        }
    }

    /// What the stage the daemon reports is doing.
    const fn of(view: &StageState) -> Self {
        match (&view.source, &view.model, view.loaded) {
            (None, ..) => Self::Empty,
            (Some(_), _, true) => Self::Running,
            (Some(_), None, false) => Self::Selected,
            (Some(_), Some(_), false) => Self::Unloaded,
        }
    }
}

/// The backend, model and device speech to text is put on.
#[derive(Debug)]
struct Goal {
    source: Option<String>,
    model: Option<String>,
    device: Option<String>,
}

/// Read once from the environment.
fn goal() -> &'static Goal {
    static GOAL: OnceLock<Goal> = OnceLock::new();
    GOAL.get_or_init(|| {
        let read = |name: &str| {
            std::env::var(super_voice_shared::SUPER_VOICE.env(name))
                .ok()
                .filter(|value| !value.is_empty())
        };
        Goal {
            source: read("APP_SCENE_STT_SOURCE"),
            model: read("APP_SCENE_STT_MODEL"),
            device: read("APP_SCENE_STT_DEVICE"),
        }
    })
}

/// One change to the transcription stage.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Change {
    Deselect,
    Select(String),
    Pick(String),
    Device(String),
    Load,
    Unload,
}

impl Change {
    fn message(&self) -> Message {
        let stage = Stage::Transcription;
        Message::Stage(match self.clone() {
            Self::Deselect => StageMessage::DeselectBackend { stage },
            Self::Select(source) => StageMessage::SelectBackend { stage, source },
            Self::Pick(model) => StageMessage::StageModel { stage, model },
            Self::Device(device) => StageMessage::StageDevice { stage, device },
            Self::Load => StageMessage::Load { stage },
            Self::Unload => StageMessage::Unload { stage },
        })
    }
}

/// How far a scene has got with speech to text.
#[derive(Debug)]
pub(super) struct Arranging {
    started: Instant,
    /// A read of the stage is on its way.
    reading: bool,
    /// The last change asked for, when, and how many times running.
    asked: Option<(Change, Instant, u32)>,
}

impl Arranging {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
            reading: false,
            asked: None,
        }
    }
}

/// What the scene does next, once the stage has been read.
enum Next {
    /// Wait for the next read.
    Wait,
    /// The stage is as the scene needs it.
    Done,
    Ask(Change),
    Fail(String),
}

impl AppModel {
    /// Read the stage, unless a read is already on its way.
    pub(super) fn read_stt(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(arranging) = self.scene.as_mut().and_then(|p| p.arranging.as_mut()) else {
            return Task::none();
        };
        if arranging.reading {
            return Task::none();
        }
        arranging.reading = true;
        Task::perform(get_stage_view(Stage::Transcription), |result| {
            cosmic::Action::App(Message::Shell(ShellMessage::SceneStt(
                result.map_err(|e| e.to_string()),
            )))
        })
    }

    /// The stage as the daemon has it: report it, or take the next step
    /// towards what the scene needs.
    pub(in crate::core::app) fn scene_stt(
        &mut self,
        read: Result<StageState, String>,
    ) -> Task<cosmic::Action<Message>> {
        let Some(progress) = self.scene.as_mut() else {
            return Task::none();
        };
        let scene = progress.scene;
        let Some(arranging) = progress.arranging.as_mut() else {
            return Task::none();
        };
        arranging.reading = false;
        let view = match read {
            Ok(view) => view,
            Err(e) => {
                return self.scene_failed(&format!("Could not read speech to text: {e}"));
            }
        };
        if scene.report {
            report(&view);
            self.scene = None;
            return Task::none();
        }
        let Some(want) = scene.stt else {
            return Task::none();
        };
        match self.next_change(scene, want, &view) {
            Next::Wait => Task::none(),
            Next::Done => {
                if let Some(progress) = self.scene.as_mut() {
                    progress.arranging = None;
                    progress.ready_at = Some(Instant::now());
                }
                Task::none()
            }
            Next::Fail(why) => self.scene_failed(&why),
            Next::Ask(change) => {
                let Some(arranging) = self.scene.as_mut().and_then(|p| p.arranging.as_mut()) else {
                    return Task::none();
                };
                let times = match &arranging.asked {
                    Some((last, _, times)) if *last == change => times + 1,
                    _ => 1,
                };
                if times > ASKS {
                    return self.scene_failed(&format!(
                        "Speech to text did not take {change:?} after {ASKS} tries, on its way to {}.",
                        want.name()
                    ));
                }
                arranging.asked = Some((change.clone(), Instant::now(), times));
                Task::done(cosmic::Action::App(change.message()))
            }
        }
    }

    /// Where `view` stands against `want`, and the change that moves it on.
    fn next_change(&self, scene: Scene, want: Stt, view: &StageState) -> Next {
        let Some(arranging) = self.scene.as_ref().and_then(|p| p.arranging.as_ref()) else {
            return Next::Wait;
        };
        if arranging.started.elapsed() > LIMIT {
            return Next::Fail(format!(
                "Speech to text was not {} after {} minutes; it is {}.",
                want.name(),
                LIMIT.as_secs() / 60,
                Stt::of(view).name()
            ));
        }
        // A change still on its way, or a load or download running: the next
        // read says where it got to.
        let taking = arranging
            .asked
            .as_ref()
            .is_some_and(|(_, at, _)| at.elapsed() < TAKE);
        if taking || !self.is_model_ready(Stage::Transcription) {
            return Next::Wait;
        }

        let goal = goal();
        if want == Stt::Empty {
            if view.source.is_none() {
                return self.once_shown(Stt::Empty, view);
            }
            return Next::Ask(Change::Deselect);
        }
        let Some(source) = goal.source.clone() else {
            return Next::Fail(format!(
                "The scene {scene:?} needs a backend: set {}.",
                super_voice_shared::SUPER_VOICE.env("APP_SCENE_STT_SOURCE")
            ));
        };
        if view.source.as_deref() != Some(source.as_str()) {
            return Next::Ask(Change::Select(source));
        }
        if want == Stt::Selected {
            // A picked model is only forgotten with the backend.
            if view.model.is_some() {
                return Next::Ask(Change::Deselect);
            }
            return self.once_shown(Stt::Selected, view);
        }

        let Some(model) = goal.model.clone() else {
            return Next::Fail(format!(
                "The scene {scene:?} needs a model: set {}.",
                super_voice_shared::SUPER_VOICE.env("APP_SCENE_STT_MODEL")
            ));
        };
        let running = view.loaded && view.model.as_deref() == Some(model.as_str());
        if running {
            if want == Stt::Unloaded {
                return Next::Ask(Change::Unload);
            }
            return self.once_shown(Stt::Running, view);
        }
        if want == Stt::Unloaded && !view.loaded && view.model.as_deref() == Some(&model) {
            return self.once_shown(Stt::Unloaded, view);
        }

        // Load it, as the card does: pick the model, wait for the daemon to
        // say what it runs on, put it on the device asked for, then Load.
        let Some(pick) = self.staged_picks.get(Stage::Transcription) else {
            return Next::Ask(Change::Pick(model));
        };
        if pick.source != source || pick.model != model {
            return Next::Ask(Change::Pick(model));
        }
        let Some(staged) = pick.device.as_deref() else {
            return Next::Wait;
        };
        let offered = self
            .device_offers
            .model(Stage::Transcription, &source, &model)
            .unwrap_or_default();
        if let Some(device) = goal.device.as_deref()
            && device != staged
            && offered.iter().any(|d| d == device)
        {
            return Next::Ask(Change::Device(device.to_string()));
        }
        Next::Ask(Change::Load)
    }

    /// `Done` once the app shows what the daemon has: the card follows the
    /// daemon's events, which can land after the read that found the stage
    /// ready.
    fn once_shown(&self, state: Stt, view: &StageState) -> Next {
        let selected = self.models_page.active_backend.as_deref() == view.source.as_deref();
        let shown = selected
            && match state {
                Stt::Running => {
                    Some(self.current_source.as_str()) == view.source.as_deref()
                        && Some(self.current_model.as_str()) == view.model.as_deref()
                }
                Stt::Empty | Stt::Selected | Stt::Unloaded => self.current_model.is_empty(),
            };
        if shown { Next::Done } else { Next::Wait }
    }

    fn scene_failed(&mut self, why: &str) -> Task<cosmic::Action<Message>> {
        println!("scene: failed: {why}");
        self.scene = None;
        Task::none()
    }
}

/// Print what the stage is doing, tab-separated: its state, backend, model and
/// device, `-` for each it has none of.
fn report(view: &StageState) {
    let or_dash = |value: Option<&str>| value.filter(|v| !v.is_empty()).unwrap_or("-").to_string();
    let device = view.device.as_ref().map(|d| d.preference.as_str());
    println!(
        "scene: stt\t{}\t{}\t{}\t{}",
        Stt::of(view).name(),
        or_dash(view.source.as_deref()),
        or_dash(view.model.as_deref()),
        or_dash(device.filter(|d| *d != "none")),
    );
}
