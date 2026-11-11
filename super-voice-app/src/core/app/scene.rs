// SPDX-License-Identifier: GPL-3.0-only
//! Opening the app on one view, for `just screenshots`.
//!
//! `SUPER_VOICE_APP_SCENE` names a page, and optionally a widget on it to click:
//! a dropdown, or the button that opens a sheet. The app goes to the page
//! itself. It does not open the widget. libcosmic opens a dropdown only on a
//! click, since its open operation is not implemented, and a sheet opened from
//! code could show a state the app never reaches, such as the backend picker
//! over a selected backend. So the app finds the widget, prints where it is,
//! and the recipe clicks there. On stdout:
//!
//! - `scene: ready` once the page is up, when there is nothing to click.
//! - `scene: click X Y` with the widget's center in window coordinates, once it
//!   has held still for [`SETTLE`].
//! - `scene: unavailable: <why>` when the widget has not appeared [`GIVE_UP`]
//!   after the page came up.
//! - `scene: failed: <why>` when speech to text could not be put in the state
//!   the scene needs.
//! - `scene: stt` and the stage's state, backend, model and device, tab
//!   separated, for the `stt` scene, which only reports.
//!
//! The daemon's state decides what the Models page shows, so a scene first puts
//! speech to text in the state its shot needs; see [`stt`]. Everything else is
//! shot as found.
//!
//! A scene opens at [`WINDOW_SIZE`], and skips single-instance activation, so
//! it opens a window of its own beside one the user may already have.
//!
//! Debug builds only. A release build ignores the variable.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use cosmic::iced::{Rectangle, Size};
use cosmic::prelude::*;
use cosmic::widget;

mod stt;

use super::AppModel;
use crate::state::{DaemonStatus, ModelsTab, Page, Section};
use crate::ui::messages::{Message, ModelsPageMessage, ShellMessage};
use stt::{Arranging, Stt};

/// The size a scene's window opens at.
pub(crate) const WINDOW_SIZE: Size = Size::new(1280.0, 800.0);

/// How often a running scene checks on its widget.
pub(super) const STEP: Duration = Duration::from_millis(250);

/// How long the widget must hold still before it is reported. Data keeps
/// arriving after the daemon connects, and a chip row that lands late moves
/// the controls below it.
const SETTLE: Duration = Duration::from_millis(1500);

/// How long after its page comes up a scene waits for its widget. The
/// daemon's answers land within a couple of seconds; a widget missing past
/// that is one the page does not draw.
const GIVE_UP: Duration = Duration::from_secs(6);

/// A view the app can open on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scene {
    page: Page,
    /// The Library page's Browse tab rather than Installed.
    browse: bool,
    target: Option<Target>,
    /// What speech to text has to be doing first; `None` takes it as found.
    stt: Option<Stt>,
    /// Print what speech to text is doing, and nothing else.
    report: bool,
}

impl Scene {
    /// `page`, with speech to text `stt`.
    const fn page(page: Page, stt: Stt) -> Self {
        Self {
            page,
            browse: false,
            target: None,
            stt: Some(stt),
            report: false,
        }
    }

    /// `page` with `target` clicked, and speech to text `stt`.
    const fn click(page: Page, target: Target, stt: Stt) -> Self {
        Self {
            target: Some(target),
            ..Self::page(page, stt)
        }
    }
}

/// The scenes, by the name `SUPER_VOICE_APP_SCENE` takes.
///
/// The Models page's follow its card from no backend to a model running; the
/// rest are shot with the model running, as the app is used.
const SCENES: &[(&str, Scene)] = &[
    ("models", Scene::page(Page::Models, Stt::Empty)),
    (
        "models/select-backend",
        Scene::click(Page::Models, Target::SelectBackend, Stt::Empty),
    ),
    (
        "models/model-menu",
        Scene::click(Page::Models, Target::ModelMenu, Stt::Selected),
    ),
    (
        "models/device-menu",
        Scene::click(Page::Models, Target::DeviceMenu, Stt::Running),
    ),
    (
        "models/language",
        Scene::click(Page::Models, Target::Language, Stt::Running),
    ),
    ("library", Scene::page(Page::Library, Stt::Running)),
    (
        "library/browse",
        Scene {
            browse: true,
            ..Scene::page(Page::Library, Stt::Running)
        },
    ),
    ("recording", Scene::page(Page::Recording, Stt::Running)),
    (
        "input-simulation",
        Scene::page(Page::InputSimulation, Stt::Running),
    ),
    ("contexts", Scene::page(Page::Contexts, Stt::Running)),
    (
        "contexts/edit",
        Scene::click(Page::Contexts, Target::EditContext, Stt::Running),
    ),
    ("speech", Scene::page(Page::Speech, Stt::Running)),
    ("voices", Scene::page(Page::Voices, Stt::Running)),
    (
        "customization",
        Scene::page(Page::Customization, Stt::Running),
    ),
    ("connection", Scene::page(Page::Connection, Stt::Running)),
    ("updates", Scene::page(Page::Updates, Stt::Running)),
    // No shot: what `just screenshots` reads first, and how it puts speech to
    // text back afterwards.
    (
        "stt",
        Scene {
            stt: None,
            report: true,
            ..Scene::page(Page::Models, Stt::Empty)
        },
    ),
    ("stt/empty", Scene::page(Page::Models, Stt::Empty)),
    ("stt/selected", Scene::page(Page::Models, Stt::Selected)),
    ("stt/unloaded", Scene::page(Page::Models, Stt::Unloaded)),
    ("stt/running", Scene::page(Page::Models, Stt::Running)),
];

/// A widget a scene clicks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// The Models page's "Select transcription backend" button.
    SelectBackend,
    /// The transcription card's model dropdown.
    ModelMenu,
    /// The transcription card's device dropdown.
    DeviceMenu,
    /// The transcription card's language button.
    Language,
    /// The first context's Edit button, on the Contexts page.
    EditContext,
}

impl Target {
    fn id(self) -> widget::Id {
        widget::Id::new(match self {
            Self::SelectBackend => "scene-select-backend",
            Self::ModelMenu => "scene-model-menu",
            Self::DeviceMenu => "scene-device-menu",
            Self::Language => "scene-language",
            Self::EditContext => "scene-edit-context",
        })
    }

    /// When the widget is drawn, for a scene that did not find it.
    const fn shown_when(self) -> &'static str {
        match self {
            Self::SelectBackend => {
                "The Select transcription backend button shows only while no backend is selected."
            }
            Self::ModelMenu => {
                "The model dropdown shows only while a backend is selected and no model is loaded."
            }
            Self::DeviceMenu => {
                "The device dropdown shows only for a model the daemon offers a device for."
            }
            Self::Language => {
                "The language button opens a picker only for a multilingual model. Load one."
            }
            Self::EditContext => {
                "The Edit button shows only once there is a context to edit. Add one."
            }
        }
    }
}

/// The scene `SUPER_VOICE_APP_SCENE` names, if any. Read once. A name that is
/// not a scene ends the process: a recipe that asked for one would otherwise
/// shoot the default view under its name.
pub(crate) fn requested() -> Option<Scene> {
    static SCENE: OnceLock<Option<Scene>> = OnceLock::new();
    *SCENE.get_or_init(from_env)
}

/// `cfg!` rather than `#[cfg]`, so a release build still compiles the scenes
/// it never opens instead of warning that nothing constructs them.
fn from_env() -> Option<Scene> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let var = super_voice_shared::SUPER_VOICE.env("APP_SCENE");
    let name = std::env::var(&var).ok().filter(|name| !name.is_empty())?;
    if let Some((_, scene)) = SCENES.iter().find(|(known, _)| *known == name) {
        return Some(*scene);
    }
    let names: Vec<&str> = SCENES.iter().map(|(known, _)| *known).collect();
    eprintln!(
        "{var}={name:?} is not a scene. The scenes are: {}",
        names.join(", ")
    );
    std::process::exit(2);
}

/// `content`, findable by a scene that clicks `target`. Left as it is unless
/// such a scene is running, so a normal launch lays out exactly as before.
pub(crate) fn tag<'a>(
    target: Target,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    if requested().is_some_and(|scene| scene.target == Some(target)) {
        widget::id_container(content, target.id()).into()
    } else {
        content.into()
    }
}

/// Where the container tagged `id` is laid out, in window coordinates.
///
/// iced's `container::visible_bounds` asks the same, but at the libcosmic
/// revision this app pins, its `traverse` calls itself rather than the closure
/// it is given, and overflows the stack. This one leaves out what that one
/// adds for scrolled content: a scene opens on a page scrolled to its top,
/// where layout coordinates are window coordinates.
fn bounds_of(id: widget::Id) -> Task<Option<Rectangle>> {
    use cosmic::iced::advanced::widget::operation::Outcome;
    use cosmic::iced::advanced::widget::{Operation, operate};

    struct Find {
        target: widget::Id,
        found: Option<Rectangle>,
    }

    impl Operation<Option<Rectangle>> for Find {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Option<Rectangle>>)) {
            operate(self);
        }

        fn container(&mut self, id: Option<&widget::Id>, bounds: Rectangle) {
            if self.found.is_none() && id == Some(&self.target) {
                self.found = Some(bounds);
            }
        }

        fn finish(&self) -> Outcome<Option<Rectangle>> {
            Outcome::Some(self.found)
        }
    }

    operate(Find {
        target: id,
        found: None,
    })
}

/// How far the running scene has got. Held on the app model until the scene
/// has printed its line.
pub(crate) struct Progress {
    scene: Scene,
    /// Speech to text on its way to what the scene needs; `None` once it is
    /// there, or for a scene that takes it as found.
    arranging: Option<Arranging>,
    /// When the page came up: the daemon connected, with speech to text as the
    /// scene needs it.
    ready_at: Option<Instant>,
    /// Where the widget was last found, and since when it has been there.
    held: Option<(Rectangle, Instant)>,
}

impl Progress {
    pub(super) fn new(scene: Scene) -> Self {
        Self {
            scene,
            arranging: (scene.stt.is_some() || scene.report).then(Arranging::new),
            ready_at: None,
            held: None,
        }
    }
}

impl AppModel {
    /// Go to the running scene's section, its page's tab there, and the
    /// Library's Browse tab if asked. Its widget waits for the daemon; see
    /// [`Self::scene_step`].
    pub(super) fn open_scene(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(scene) = self.scene.as_ref().map(|progress| progress.scene) else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        let section = Section::of(scene.page);
        let entry = self
            .nav
            .iter()
            .find(|&id| self.nav.data::<Section>(id) == Some(&section));
        if let Some(id) = entry {
            tasks.push(cosmic::Application::on_nav_select(self, id));
        }
        // A feature's section opens on the tab it last showed; the scene's
        // page is one of its tabs.
        if section.pages().len() > 1 {
            tasks.push(Task::done(cosmic::Action::App(Message::Shell(
                ShellMessage::SelectTab(scene.page),
            ))));
        }
        let tabs = &self.models_page.models_tabs;
        let browse = tabs
            .iter()
            .find(|&id| tabs.data::<ModelsTab>(id) == Some(&ModelsTab::Download));
        if scene.browse
            && let Some(id) = browse
        {
            tasks.push(Task::done(cosmic::Action::App(Message::ModelsPage(
                ModelsPageMessage::ModelsTabActivated(id),
            ))));
        }
        Task::batch(tasks)
    }

    /// One check on the running scene. Once the daemon is connected, put
    /// speech to text as the scene needs it, then report ready, or look for the
    /// widget to click.
    pub(super) fn scene_step(&mut self) -> Task<cosmic::Action<Message>> {
        let connected = self.daemon_status == DaemonStatus::Connected;
        let Some(progress) = self.scene.as_mut() else {
            return Task::none();
        };
        if !connected {
            return Task::none();
        }
        if progress.arranging.is_some() {
            return self.read_stt();
        }
        let ready_at = *progress.ready_at.get_or_insert_with(Instant::now);
        let Some(target) = progress.scene.target else {
            println!("scene: ready");
            self.scene = None;
            return Task::none();
        };
        if ready_at.elapsed() > GIVE_UP {
            println!("scene: unavailable: {}", target.shown_when());
            self.scene = None;
            return Task::none();
        }
        bounds_of(target.id())
            .map(|bounds| cosmic::Action::App(Message::Shell(ShellMessage::SceneTarget(bounds))))
    }

    /// Where the scene's widget is drawn, if it is. Printed once it has held
    /// still for [`SETTLE`].
    pub(super) fn scene_target(
        &mut self,
        bounds: Option<Rectangle>,
    ) -> Task<cosmic::Action<Message>> {
        let Some(progress) = self.scene.as_mut() else {
            return Task::none();
        };
        let Some(bounds) = bounds else {
            progress.held = None;
            return Task::none();
        };
        match progress.held {
            Some((held, since)) if held == bounds => {
                if since.elapsed() >= SETTLE {
                    let center = bounds.center();
                    println!("scene: click {} {}", center.x.round(), center.y.round());
                    self.scene = None;
                }
            }
            _ => progress.held = Some((bounds, Instant::now())),
        }
        Task::none()
    }
}
