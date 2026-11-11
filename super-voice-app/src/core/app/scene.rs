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
//! - `scene: ready` once the daemon is connected, when there is nothing to click.
//! - `scene: click X Y` with the widget's center in window coordinates, once it
//!   has held still for [`SETTLE`].
//! - `scene: unavailable: <why>` when the widget has not appeared [`GIVE_UP`]
//!   after connecting. The daemon's state decides what a page shows, and a
//!   scene never changes that state.
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

use super::AppModel;
use crate::state::{DaemonStatus, ModelsTab, Page, Section};
use crate::ui::messages::{Message, ModelsPageMessage, ShellMessage};

/// The size a scene's window opens at.
pub(crate) const WINDOW_SIZE: Size = Size::new(1280.0, 800.0);

/// How often a running scene checks on its widget.
pub(super) const STEP: Duration = Duration::from_millis(250);

/// How long the widget must hold still before it is reported. Data keeps
/// arriving after the daemon connects, and a chip row that lands late moves
/// the controls below it.
const SETTLE: Duration = Duration::from_millis(1500);

/// How long after connecting a scene waits for its widget. The daemon's
/// answers land within a couple of seconds of connecting; past that, a widget
/// that has not appeared is one the state rules out, and every skipped shot
/// costs this much.
const GIVE_UP: Duration = Duration::from_secs(6);

/// A view the app can open on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scene {
    page: Page,
    /// The Library page's Browse tab rather than Installed.
    browse: bool,
    target: Option<Target>,
}

impl Scene {
    const fn page(page: Page) -> Self {
        Self {
            page,
            browse: false,
            target: None,
        }
    }

    const fn click(page: Page, target: Target) -> Self {
        Self {
            page,
            browse: false,
            target: Some(target),
        }
    }
}

/// The scenes, by the name `SUPER_VOICE_APP_SCENE` takes.
const SCENES: &[(&str, Scene)] = &[
    ("models", Scene::page(Page::Models)),
    (
        "models/select-backend",
        Scene::click(Page::Models, Target::SelectBackend),
    ),
    (
        "models/model-menu",
        Scene::click(Page::Models, Target::ModelMenu),
    ),
    (
        "models/device-menu",
        Scene::click(Page::Models, Target::DeviceMenu),
    ),
    (
        "models/language",
        Scene::click(Page::Models, Target::Language),
    ),
    ("library", Scene::page(Page::Library)),
    (
        "library/browse",
        Scene {
            page: Page::Library,
            browse: true,
            target: None,
        },
    ),
    ("recording", Scene::page(Page::Recording)),
    ("input-simulation", Scene::page(Page::InputSimulation)),
    ("contexts", Scene::page(Page::Contexts)),
    (
        "contexts/edit",
        Scene::click(Page::Contexts, Target::EditContext),
    ),
    ("speech", Scene::page(Page::Speech)),
    ("voices", Scene::page(Page::Voices)),
    ("customization", Scene::page(Page::Customization)),
    ("connection", Scene::page(Page::Connection)),
    ("updates", Scene::page(Page::Updates)),
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
            Self::Language => "The language button opens a picker only for a multilingual model.",
            Self::EditContext => "The Edit button shows only once there is a context to edit.",
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
    /// When the daemon was first seen connected.
    connected_at: Option<Instant>,
    /// Where the widget was last found, and since when it has been there.
    held: Option<(Rectangle, Instant)>,
}

impl Progress {
    pub(super) const fn new(scene: Scene) -> Self {
        Self {
            scene,
            connected_at: None,
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

    /// One check on the running scene. Once the daemon is connected, report
    /// ready, or look for the widget to click.
    pub(super) fn scene_step(&mut self) -> Task<cosmic::Action<Message>> {
        let connected = self.daemon_status == DaemonStatus::Connected;
        let Some(progress) = self.scene.as_mut() else {
            return Task::none();
        };
        if !connected {
            return Task::none();
        }
        let connected_at = *progress.connected_at.get_or_insert_with(Instant::now);
        let Some(target) = progress.scene.target else {
            println!("scene: ready");
            self.scene = None;
            return Task::none();
        };
        if connected_at.elapsed() > GIVE_UP {
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
