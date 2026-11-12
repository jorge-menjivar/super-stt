// SPDX-License-Identifier: GPL-3.0-only
//! Super Voice's keyboard shortcuts, which the daemon owns: `/v1/shortcuts`.
//!
//! The daemon owns them so every client sees the same answer, and so a client
//! that cannot write desktop settings, like a sandboxed app, can still add
//! one. How a shortcut gets onto the desktop depends on the desktop:
//!
//! 1. COSMIC: an entry in its custom-shortcuts file
//!    ([`super_voice_shared::cosmic_shortcuts`]), which runs `voice`.
//! 2. A desktop with the `GlobalShortcuts` portal, such as KDE Plasma, GNOME
//!    or Hyprland: the daemon binds it in a portal session it holds
//!    ([`portal`]), and runs the same `voice` when it is pressed. The ones
//!    the user added are in the config, and bound again at every start.
//! 3. Anything else: the user binds the command in the desktop's settings.
//!
//! Contract: `docs/protocol/endpoints/v1/shortcuts.md`.

#[cfg(target_os = "linux")]
pub mod portal;

use log::{info, warn};
use super_voice_shared::cosmic_shortcuts::{self, Status};
use super_voice_shared::models::shortcuts::{
    Shortcut, ShortcutId, ShortcutList, ShortcutMethod, ShortcutState,
};

/// Why a shortcut could not be added or removed.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Another shortcut holds its keys.
    KeysTaken,
    /// The user said no in the desktop's dialog.
    Refused,
    /// This desktop has no method the daemon can add it with.
    Unsupported,
    /// The desktop's settings could not be changed.
    Write(String),
}

/// One shortcut the desktop has bound through the portal, and its keys as the
/// desktop describes them.
#[cfg(target_os = "linux")]
pub use portal::Bound;

/// One shortcut the desktop has bound through the portal. Only Linux has the
/// portal, so elsewhere there never is one.
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    pub id: ShortcutId,
    pub keys: String,
}

/// The daemon's hold on the desktop's shortcuts: how it puts them there, and
/// on a desktop with the portal, the session that holds them.
#[derive(Default)]
pub struct Shortcuts {
    /// The method, once one is found. `manual` is not kept: the portal may
    /// only have been slow to start, and asking again is cheap.
    method: std::sync::OnceLock<ShortcutMethod>,
    /// Held across a whole change, so two at once cannot each bind the set
    /// the other is replacing.
    changing: tokio::sync::Mutex<()>,
    #[cfg(target_os = "linux")]
    session: tokio::sync::Mutex<Option<portal::Session>>,
}

impl Shortcuts {
    /// How the daemon puts shortcuts on the desktop it runs in. Only a Linux
    /// desktop takes them from the daemon: anywhere else it is `manual`.
    // `allow`, not `expect`: clippy 1.98 files these under `unused_async_trait_impl`
    // and later releases under `unused_async`, so either `expect` goes unfulfilled
    // on some toolchain.
    #[cfg_attr(
        not(target_os = "linux"),
        allow(
            clippy::unused_async,
            clippy::unused_async_trait_impl,
            reason = "detecting COSMIC and the portal is Linux's: elsewhere there is nothing to await"
        )
    )]
    pub async fn method(&self) -> ShortcutMethod {
        if let Some(method) = self.method.get() {
            return *method;
        }
        #[cfg(target_os = "linux")]
        let method = detect_method().await;
        #[cfg(not(target_os = "linux"))]
        let method = ShortcutMethod::Manual;
        if method == ShortcutMethod::Manual {
            log::debug!("Keyboard shortcuts: manual");
        } else if self.method.set(method).is_ok() {
            info!("Keyboard shortcuts: {method}");
        }
        method
    }

    /// Taken across a change to the shortcuts.
    pub async fn changing(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.changing.lock().await
    }

    /// Every shortcut and where it stands.
    pub async fn list(&self) -> ShortcutList {
        let method = self.method().await;
        let cosmic_file = if method == ShortcutMethod::Cosmic {
            tokio::task::spawn_blocking(cosmic_file)
                .await
                .unwrap_or_default()
        } else {
            String::new()
        };
        let bound = self.bound().await;
        list_from(method, &cosmic_file, &bound)
    }

    /// The shortcuts bound through the portal now: none without a session.
    // `allow`, not `expect`: clippy 1.98 files these under `unused_async_trait_impl`
    // and later releases under `unused_async`, so either `expect` goes unfulfilled
    // on some toolchain.
    #[cfg_attr(
        not(target_os = "linux"),
        allow(
            clippy::unused_async,
            clippy::unused_async_trait_impl,
            reason = "the portal session it awaits is Linux's"
        )
    )]
    async fn bound(&self) -> Vec<Bound> {
        #[cfg(target_os = "linux")]
        if let Some(session) = self.session.lock().await.as_ref() {
            return session.list().await.unwrap_or_else(|e| {
                warn!("Could not list the bound shortcuts: {e}");
                Vec::new()
            });
        }
        Vec::new()
    }

    /// Bind exactly `set` through the portal, in a new session that replaces
    /// the one held, or close it when `set` is empty. Answers what the desktop
    /// bound.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] when the user turned every one down,
    /// [`Error::Write`] when the portal failed, and [`Error::Unsupported`]
    /// where there is no portal.
    // `allow`, not `expect`: clippy 1.98 files these under `unused_async_trait_impl`
    // and later releases under `unused_async`, so either `expect` goes unfulfilled
    // on some toolchain.
    #[cfg_attr(
        not(target_os = "linux"),
        allow(
            clippy::unused_async,
            clippy::unused_async_trait_impl,
            reason = "the portal session it awaits is Linux's"
        )
    )]
    pub async fn bind(&self, set: &[ShortcutId]) -> Result<Vec<Bound>, Error> {
        #[cfg(target_os = "linux")]
        {
            let mut held = self.session.lock().await;
            if let Some(old) = held.take() {
                old.close().await;
            }
            if set.is_empty() {
                return Ok(Vec::new());
            }
            let session = open_session().await?;
            let bound = session.bind(set).await.map_err(Error::from)?;
            *held = Some(session);
            Ok(bound)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = set;
            Err(Error::Unsupported)
        }
    }

    /// Bind the shortcuts `saved` at a start, and answer the ones kept: one
    /// the user removed in the desktop's settings is not, so the page offers
    /// to add it again.
    ///
    /// 1. KDE Plasma lists, in a new session, the shortcuts it kept, so only
    ///    those are bound.
    /// 2. GNOME lists nothing until a bind, and Hyprland keeps nothing, so
    ///    every saved one is bound. GNOME asks again, in its dialog, for one
    ///    removed in its settings: one the user turns down there is not kept.
    ///
    /// # Errors
    ///
    /// As [`Shortcuts::bind`].
    // `allow`, not `expect`: clippy 1.98 files these under `unused_async_trait_impl`
    // and later releases under `unused_async`, so either `expect` goes unfulfilled
    // on some toolchain.
    #[cfg_attr(
        not(target_os = "linux"),
        allow(
            clippy::unused_async,
            clippy::unused_async_trait_impl,
            reason = "the portal session it awaits is Linux's"
        )
    )]
    pub async fn bind_saved(&self, saved: &[ShortcutId]) -> Result<Vec<ShortcutId>, Error> {
        #[cfg(target_os = "linux")]
        {
            let mut held = self.session.lock().await;
            if let Some(old) = held.take() {
                old.close().await;
            }
            let session = open_session().await?;
            let asked =
                if lists_kept_shortcuts(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref()) {
                    match session.list().await {
                        Ok(listed) => still_kept(saved, &listed),
                        Err(e) => {
                            warn!("Could not ask which shortcuts the desktop kept: {e:#}");
                            saved.to_vec()
                        }
                    }
                } else {
                    saved.to_vec()
                };
            if asked.is_empty() {
                session.close().await;
                return Ok(asked);
            }
            match session.bind(&asked).await {
                Ok(bound) => {
                    *held = Some(session);
                    Ok(still_kept(&asked, &bound))
                }
                Err(portal::BindError::Cancelled) => {
                    session.close().await;
                    Ok(Vec::new())
                }
                Err(e) => Err(e.into()),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = saved;
            Err(Error::Unsupported)
        }
    }
}

#[cfg(target_os = "linux")]
impl From<portal::BindError> for Error {
    fn from(e: portal::BindError) -> Self {
        match e {
            portal::BindError::Cancelled => Self::Refused,
            portal::BindError::Failed(e) => Self::Write(format!("{e:#}")),
        }
    }
}

/// A portal session on a connection of its own: the app id registered on it
/// is the portal's first call there. Each press runs the shortcut's command.
#[cfg(target_os = "linux")]
async fn open_session() -> Result<portal::Session, Error> {
    let conn = async { zbus::connection::Builder::session()?.build().await }
        .await
        .map_err(|e: zbus::Error| Error::Write(e.to_string()))?;
    portal::Session::open(conn, run)
        .await
        .map_err(|e| Error::Write(format!("{e:#}")))
}

/// The ones of `saved` the desktop still lists.
#[cfg(target_os = "linux")]
fn still_kept(saved: &[ShortcutId], listed: &[Bound]) -> Vec<ShortcutId> {
    saved
        .iter()
        .copied()
        .filter(|id| listed.iter().any(|b| b.id == *id))
        .collect()
}

/// [`Shortcuts::method`] on Linux: COSMIC by its name in
/// `XDG_CURRENT_DESKTOP`, then the portal when the desktop offers
/// `GlobalShortcuts`.
#[cfg(target_os = "linux")]
async fn detect_method() -> ShortcutMethod {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok();
    if is_cosmic(desktop.as_deref()) {
        ShortcutMethod::Cosmic
    } else {
        method_for(desktop.as_deref(), portal_available().await)
    }
}

#[cfg(target_os = "linux")]
async fn portal_available() -> bool {
    match zbus::Connection::session().await {
        Ok(conn) => portal::available(&conn).await,
        Err(e) => {
            log::debug!("GlobalShortcuts portal: no session bus ({e})");
            false
        }
    }
}

/// Whether `XDG_CURRENT_DESKTOP`, a `:`-separated list, names `name`.
#[cfg(target_os = "linux")]
fn names(desktop: Option<&str>, name: &str) -> bool {
    desktop.is_some_and(|desktop| desktop.split(':').any(|d| d.eq_ignore_ascii_case(name)))
}

/// Whether `XDG_CURRENT_DESKTOP` names COSMIC.
#[cfg(target_os = "linux")]
fn is_cosmic(desktop: Option<&str>) -> bool {
    names(desktop, "COSMIC")
}

/// Whether the desktop lists, in a new session and before anything is bound
/// in it, the shortcuts it kept from the app's sessions before. KDE Plasma
/// does. GNOME lists nothing until a bind, and Hyprland keeps nothing.
#[cfg(target_os = "linux")]
fn lists_kept_shortcuts(desktop: Option<&str>) -> bool {
    names(desktop, "KDE")
}

/// The method for an `XDG_CURRENT_DESKTOP`, given whether the desktop offers
/// the `GlobalShortcuts` portal. COSMIC's own file comes first: COSMIC has no
/// such portal, but another desktop's backend for it may be installed.
#[cfg(target_os = "linux")]
fn method_for(desktop: Option<&str>, portal: bool) -> ShortcutMethod {
    if is_cosmic(desktop) {
        ShortcutMethod::Cosmic
    } else if portal {
        ShortcutMethod::Portal
    } else {
        ShortcutMethod::Manual
    }
}

/// The COSMIC shortcut `id` is.
fn cosmic(id: ShortcutId) -> &'static cosmic_shortcuts::Shortcut {
    match id {
        ShortcutId::Dictate => &cosmic_shortcuts::DICTATE,
        ShortcutId::ReadAloud => &cosmic_shortcuts::READ_ALOUD,
    }
}

/// The `voice` a shortcut the daemon adds runs: the one installed beside the
/// daemon, or the one on `PATH` when there is none, as in a development
/// build.
fn voice_program() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("voice")))
        .filter(|voice| voice.is_file())
        .map_or_else(|| "voice".to_string(), |voice| voice.display().to_string())
}

/// Run what shortcut `id` runs, as COSMIC's entry for it does: `voice` with
/// its arguments, which starts it or, pressed again, stops it. What `voice`
/// says when it fails is logged.
pub fn run(id: ShortcutId) {
    let args = cosmic(id).args;
    info!("Shortcut {id} pressed: voice {args}");
    let command = tokio::process::Command::new(voice_program())
        .args(args.split_whitespace())
        .stdin(std::process::Stdio::null())
        .output();
    tokio::spawn(async move {
        match command.await {
            Ok(out) if out.status.success() => {}
            Ok(out) => warn!(
                "voice {args} failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => warn!("Could not run voice {args}: {e}"),
        }
    });
}

/// Every shortcut and where it stands, given the method, COSMIC's shortcuts
/// file's text for COSMIC, and what the portal has bound for the portal.
#[must_use]
pub fn list_from(method: ShortcutMethod, cosmic_file: &str, bound: &[Bound]) -> ShortcutList {
    let shortcuts = ShortcutId::ALL
        .into_iter()
        .map(|id| {
            let shortcut = cosmic(id);
            let (state, keys) = match method {
                ShortcutMethod::Cosmic => match cosmic_shortcuts::status(cosmic_file, shortcut) {
                    Status::Installed { keys } => (ShortcutState::Installed, keys),
                    Status::Missing => (ShortcutState::Missing, shortcut.keys()),
                    Status::Taken => (ShortcutState::Taken, shortcut.keys()),
                },
                ShortcutMethod::Portal => match bound.iter().find(|b| b.id == id) {
                    Some(bound) => (ShortcutState::Installed, bound.keys.clone()),
                    None => (ShortcutState::Missing, shortcut.keys()),
                },
                ShortcutMethod::Manual => (ShortcutState::Manual, shortcut.keys()),
            };
            Shortcut {
                id,
                state,
                keys,
                command: format!("voice {}", shortcut.args),
            }
        })
        .collect();
    ShortcutList {
        status: "success".to_string(),
        method,
        shortcuts,
    }
}

/// COSMIC's shortcuts file's text, or nothing when there is none yet.
fn cosmic_file() -> String {
    cosmic_shortcuts::file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .unwrap_or_default()
}

/// Add shortcut `id` to COSMIC's shortcuts file. Adding one that is set up
/// already changes nothing and succeeds. Reads and writes the file, so call
/// it off the async runtime.
///
/// # Errors
///
/// [`Error::KeysTaken`] when another shortcut holds its keys, and
/// [`Error::Write`] when the file could not be written.
pub fn add_cosmic(id: ShortcutId) -> Result<(), Error> {
    let shortcut = cosmic(id);
    if cosmic_shortcuts::status(&cosmic_file(), shortcut) == Status::Taken {
        return Err(Error::KeysTaken);
    }
    let command = format!("{} {}", voice_program(), shortcut.args);
    cosmic_shortcuts::add(shortcut, &command)
        .map(|_| ())
        .map_err(|e| Error::Write(e.to_string()))
}

/// Remove shortcut `id` from COSMIC's shortcuts file. Removing one that is
/// not set up changes nothing and succeeds. Call it off the async runtime.
///
/// # Errors
///
/// [`Error::Write`] when the file could not be written.
pub fn remove_cosmic(id: ShortcutId) -> Result<(), Error> {
    cosmic_shortcuts::remove(cosmic(id))
        .map(|_| ())
        .map_err(|e| Error::Write(e.to_string()))
}

/// `set` with `id` added or taken out, in the order a list shows them.
#[must_use]
pub fn with(set: &[ShortcutId], id: ShortcutId, present: bool) -> Vec<ShortcutId> {
    ShortcutId::ALL
        .into_iter()
        .filter(|s| if *s == id { present } else { set.contains(s) })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn cosmic_is_found_in_a_desktop_list() {
        assert_eq!(method_for(Some("COSMIC"), false), ShortcutMethod::Cosmic);
        assert_eq!(
            method_for(Some("pop:COSMIC"), false),
            ShortcutMethod::Cosmic
        );
        assert_eq!(method_for(Some("KDE"), false), ShortcutMethod::Manual);
        assert_eq!(method_for(Some(""), false), ShortcutMethod::Manual);
        assert_eq!(method_for(None, false), ShortcutMethod::Manual);
    }

    /// The portal is used wherever the desktop offers it, except on COSMIC,
    /// whose own file comes first.
    /// Only KDE Plasma says, before a bind, which shortcuts it kept.
    #[cfg(target_os = "linux")]
    #[test]
    fn only_kde_lists_the_shortcuts_it_kept() {
        assert!(lists_kept_shortcuts(Some("KDE")));
        assert!(lists_kept_shortcuts(Some("kde:wayland")));
        assert!(!lists_kept_shortcuts(Some("GNOME")));
        assert!(!lists_kept_shortcuts(Some("Hyprland")));
        assert!(!lists_kept_shortcuts(None));
    }

    /// A saved shortcut the desktop no longer lists was removed in its
    /// settings, and is not kept.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_shortcut_the_desktop_dropped_is_not_kept() {
        use ShortcutId::{Dictate, ReadAloud};
        let listed = [Bound {
            id: ReadAloud,
            keys: String::new(),
        }];
        assert_eq!(still_kept(&[Dictate, ReadAloud], &listed), [ReadAloud]);
        assert_eq!(still_kept(&[Dictate], &[]), []);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_portal_is_used_where_it_is_offered() {
        assert_eq!(method_for(Some("KDE"), true), ShortcutMethod::Portal);
        assert_eq!(method_for(Some("GNOME"), true), ShortcutMethod::Portal);
        assert_eq!(method_for(Some("COSMIC"), true), ShortcutMethod::Cosmic);
    }

    /// On COSMIC each shortcut reads from the file: set up on its keys,
    /// missing, or blocked. Elsewhere both are the user's to bind.
    #[test]
    fn the_list_says_where_each_stands() {
        let file = r#"{ (modifiers: [Super], key: "space", description: Some("Super Voice")): Spawn("/usr/local/bin/voice record --write"), (modifiers: [Super, Shift], key: "space"): Spawn("x"), }"#;
        let list = list_from(ShortcutMethod::Cosmic, file, &[]);
        assert_eq!(list.method, ShortcutMethod::Cosmic);
        let [dictate, read] = list.shortcuts.as_slice() else {
            panic!("two shortcuts: {list:?}");
        };
        assert_eq!(
            (dictate.id, dictate.state, dictate.keys.as_str()),
            (ShortcutId::Dictate, ShortcutState::Installed, "Super+Space")
        );
        assert_eq!(
            (read.id, read.state, read.keys.as_str()),
            (
                ShortcutId::ReadAloud,
                ShortcutState::Taken,
                "Super+Shift+Space"
            )
        );
        assert_eq!(read.command, "voice speak --read");

        let missing = list_from(ShortcutMethod::Cosmic, "", &[]);
        assert!(
            missing
                .shortcuts
                .iter()
                .all(|s| s.state == ShortcutState::Missing)
        );

        let manual = list_from(ShortcutMethod::Manual, file, &[]);
        assert!(
            manual
                .shortcuts
                .iter()
                .all(|s| s.state == ShortcutState::Manual)
        );
    }

    /// Through the portal, a shortcut is set up when the desktop bound it, on
    /// the keys the desktop says, and missing otherwise.
    #[test]
    fn the_portal_list_is_what_the_desktop_bound() {
        let bound = [Bound {
            id: ShortcutId::ReadAloud,
            keys: "Meta+Shift+Space".to_string(),
        }];
        let list = list_from(ShortcutMethod::Portal, "", &bound);
        let [dictate, read] = list.shortcuts.as_slice() else {
            panic!("two shortcuts: {list:?}");
        };
        assert_eq!(
            (dictate.state, dictate.keys.as_str()),
            (ShortcutState::Missing, "Super+Space")
        );
        assert_eq!(
            (read.state, read.keys.as_str()),
            (ShortcutState::Installed, "Meta+Shift+Space")
        );
    }

    #[test]
    fn a_set_keeps_the_list_order() {
        use ShortcutId::{Dictate, ReadAloud};
        assert_eq!(with(&[ReadAloud], Dictate, true), [Dictate, ReadAloud]);
        assert_eq!(with(&[Dictate, ReadAloud], Dictate, false), [ReadAloud]);
        assert_eq!(with(&[], ReadAloud, false), []);
        assert_eq!(with(&[ReadAloud], ReadAloud, true), [ReadAloud]);
    }
}
