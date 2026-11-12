// SPDX-License-Identifier: GPL-3.0-only
//! The `GlobalShortcuts` portal: how the daemon puts its shortcuts on a
//! desktop that offers it, such as KDE Plasma, GNOME and Hyprland.
//!
//! The portal binds shortcuts to a session, and a session lasts only as long
//! as the connection that opened it. So the daemon holds one open, binds in it
//! every shortcut the user added, and binds them again at every start. A press
//! arrives as `Activated` on that session. Nothing unbinds a shortcut: the
//! daemon closes the session and binds the ones left in a new one, and the
//! desktop may still list a dropped one in its settings.
//!
//! A desktop that keeps its shortcuts, as KDE Plasma and GNOME do, lists in a
//! new session the ones it kept, before anything is bound in it. A shortcut
//! the daemon saved but the desktop no longer has was removed in the
//! desktop's settings, and is not bound again.
//!
//! The portal asks who is calling by app id, and the daemon, a service the
//! host runs, has none. So it first registers the app's, [`APP_ID`], on the
//! connection, and the desktop names Super Voice in its dialog and in its
//! shortcut settings. A portal older than 1.21 has no registry and finds the
//! id itself.

use crate::portal::{
    PORTAL_BUS, PORTAL_PATH, RESPONSE_CANCELLED, next_handle_token, portal_request,
};
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use log::{debug, info, warn};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use super_voice_shared::cosmic_shortcuts;
use super_voice_shared::models::shortcuts::ShortcutId;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const GLOBAL_SHORTCUTS_IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const REGISTRY_IFACE: &str = "org.freedesktop.host.portal.Registry";
const SESSION_IFACE: &str = "org.freedesktop.portal.Session";

/// The app id the daemon registers: the settings app's, from its
/// `super-voice-app.desktop`.
pub const APP_ID: &str = "super-voice-app";

/// How long a call may take, other than a bind.
const TIMEOUT_SECS: u64 = 10;

/// How long a bind may take. The desktop can ask the user first, in a dialog,
/// and a client's request waits on the answer, up to its own 90 s.
const BIND_TIMEOUT_SECS: u64 = 60;

/// One shortcut the desktop has bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    pub id: ShortcutId,
    /// Its keys, as the desktop describes them, like `Meta+Space`. Empty when
    /// it does not say, as on Hyprland, where the user binds the keys in its
    /// config.
    pub keys: String,
}

/// Why a bind came to nothing.
#[derive(Debug)]
pub enum BindError {
    /// The user said no in the desktop's dialog.
    Cancelled,
    /// The portal failed, or could not be reached.
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for BindError {
    fn from(e: anyhow::Error) -> Self {
        Self::Failed(e)
    }
}

/// Whether the desktop offers `GlobalShortcuts`. Asks the portal, which
/// starts it when it is not running yet.
pub async fn available(conn: &zbus::Connection) -> bool {
    let Ok(proxy) = zbus::Proxy::new(conn, PORTAL_BUS, PORTAL_PATH, GLOBAL_SHORTCUTS_IFACE).await
    else {
        return false;
    };
    match tokio::time::timeout(
        Duration::from_secs(TIMEOUT_SECS),
        proxy.get_property::<u32>("version"),
    )
    .await
    {
        Ok(Ok(version)) => {
            debug!("GlobalShortcuts portal: version {version}");
            true
        }
        Ok(Err(e)) => {
            debug!("GlobalShortcuts portal: not offered ({e})");
            false
        }
        Err(_) => {
            debug!("GlobalShortcuts portal: no answer");
            false
        }
    }
}

/// A session holding Super Voice's shortcuts. Closed when dropped, since the
/// connection that opened it goes with it.
pub struct Session {
    conn: zbus::Connection,
    handle: OwnedObjectPath,
    /// Hands each press on to the caller of [`Session::open`].
    presses: tokio::task::JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.presses.abort();
    }
}

impl Session {
    /// Open a session on `conn`, a connection of its own. Each press of a
    /// shortcut later bound in it is handed to `pressed`.
    ///
    /// # Errors
    ///
    /// When the portal fails.
    pub async fn open(
        conn: zbus::Connection,
        pressed: impl Fn(ShortcutId) + Send + Sync + 'static,
    ) -> Result<Self> {
        register(&conn).await;
        let portal = zbus::Proxy::new(&conn, PORTAL_BUS, PORTAL_PATH, GLOBAL_SHORTCUTS_IFACE)
            .await
            .context("Failed to create GlobalShortcuts proxy")?;
        // Listening before the session exists, so no press is missed.
        let mut activated = portal
            .receive_signal("Activated")
            .await
            .context("Failed to listen for shortcut presses")?;
        let handle = create_session(&conn, &portal).await?;

        let ours = handle.clone();
        let listener = tokio::spawn(async move {
            while let Some(signal) = activated.next().await {
                let body = signal.body();
                let Ok((session, id, _, _)) = body.deserialize::<(
                    OwnedObjectPath,
                    String,
                    u64,
                    HashMap<String, OwnedValue>,
                )>() else {
                    continue;
                };
                if session != ours {
                    continue;
                }
                match id.parse::<ShortcutId>() {
                    Ok(id) => pressed(id),
                    Err(e) => debug!("GlobalShortcuts portal: press of {e}"),
                }
            }
        });
        Ok(Self {
            conn,
            handle,
            presses: listener,
        })
    }

    /// Bind `shortcuts` in this session. Answers what the desktop bound,
    /// which is not every shortcut asked for when the user turned one down.
    ///
    /// # Errors
    ///
    /// [`BindError::Cancelled`] when the user turned down every shortcut,
    /// and [`BindError::Failed`] when the portal failed.
    pub async fn bind(&self, shortcuts: &[ShortcutId]) -> Result<Vec<Bound>, BindError> {
        let portal = zbus::Proxy::new(&self.conn, PORTAL_BUS, PORTAL_PATH, GLOBAL_SHORTCUTS_IFACE)
            .await
            .context("Failed to create GlobalShortcuts proxy")?;
        self.bind_shortcuts(&portal, shortcuts).await
    }

    /// `BindShortcuts`, answering what the desktop bound.
    ///
    /// GNOME 48 answers a bind that worked as one that failed, so a bind that
    /// fails is checked against `ListShortcuts` before it is believed.
    async fn bind_shortcuts(
        &self,
        portal: &zbus::Proxy<'_>,
        shortcuts: &[ShortcutId],
    ) -> Result<Vec<Bound>, BindError> {
        let token = next_handle_token();
        let mut options = HashMap::<&str, Value<'_>>::new();
        options.insert("handle_token", Value::from(token.as_str()));
        let (code, results) = portal_request(
            &self.conn,
            portal,
            "BindShortcuts",
            &token,
            &(self.handle.as_ref(), bind_args(shortcuts), "", options),
            BIND_TIMEOUT_SECS,
        )
        .await?;
        if code == 0 {
            let bound = bound_from(&results);
            info!("Bound {} of {} shortcuts", bound.len(), shortcuts.len());
            return Ok(bound);
        }
        let bound = self.list().await.unwrap_or_default();
        if !bound.is_empty() {
            warn!("BindShortcuts answered {code}, but the shortcuts are bound");
            return Ok(bound);
        }
        if code == RESPONSE_CANCELLED {
            return Err(BindError::Cancelled);
        }
        Err(BindError::Failed(anyhow::anyhow!(
            "BindShortcuts failed (response code {code})"
        )))
    }

    /// The shortcuts bound in this session, on the keys the desktop has
    /// them on now: the user may have changed them in its settings.
    ///
    /// # Errors
    ///
    /// When the portal fails.
    pub async fn list(&self) -> Result<Vec<Bound>> {
        let portal =
            zbus::Proxy::new(&self.conn, PORTAL_BUS, PORTAL_PATH, GLOBAL_SHORTCUTS_IFACE).await?;
        let token = next_handle_token();
        let mut options = HashMap::<&str, Value<'_>>::new();
        options.insert("handle_token", Value::from(token.as_str()));
        let (code, results) = portal_request(
            &self.conn,
            &portal,
            "ListShortcuts",
            &token,
            &(self.handle.as_ref(), options),
            TIMEOUT_SECS,
        )
        .await?;
        if code != 0 {
            bail!("ListShortcuts failed (response code {code})");
        }
        Ok(bound_from(&results))
    }

    /// Close the session, unbinding its shortcuts.
    pub async fn close(self) {
        let closed = async {
            let session =
                zbus::Proxy::new(&self.conn, PORTAL_BUS, self.handle.as_ref(), SESSION_IFACE)
                    .await?;
            session.call::<_, _, ()>("Close", &()).await
        };
        if let Err(e) = closed.await {
            debug!("GlobalShortcuts portal: closing {}: {e}", self.handle);
        }
    }
}

/// Register [`APP_ID`] as who `conn` calls the portal for. Must come before
/// any other portal call on it.
async fn register(conn: &zbus::Connection) {
    let registered = async {
        let registry = zbus::Proxy::new(conn, PORTAL_BUS, PORTAL_PATH, REGISTRY_IFACE).await?;
        registry
            .call::<_, _, ()>("Register", &(APP_ID, HashMap::<&str, Value<'_>>::new()))
            .await
    };
    match registered.await {
        Ok(()) => debug!("GlobalShortcuts portal: registered as {APP_ID}"),
        Err(e) => debug!("GlobalShortcuts portal: could not register as {APP_ID}: {e}"),
    }
}

/// A `session_handle_token` no other session from this process is using.
fn next_session_token() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "supervoice_shortcuts{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

async fn create_session(
    conn: &zbus::Connection,
    portal: &zbus::Proxy<'_>,
) -> Result<OwnedObjectPath> {
    let token = next_handle_token();
    let session_token = next_session_token();
    let mut options = HashMap::<&str, Value<'_>>::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert("session_handle_token", Value::from(session_token.as_str()));
    let (code, results) = portal_request(
        conn,
        portal,
        "CreateSession",
        &token,
        &(options,),
        TIMEOUT_SECS,
    )
    .await?;
    if code != 0 {
        bail!("CreateSession failed (response code {code})");
    }
    let handle = results
        .get("session_handle")
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        .and_then(|s| OwnedObjectPath::try_from(s).ok())
        .context("No session_handle in CreateSession response")?;
    debug!("GlobalShortcuts session: {handle}");
    Ok(handle)
}

/// The shortcut `id` is, with the keys it binds on COSMIC.
fn shortcut(id: ShortcutId) -> &'static cosmic_shortcuts::Shortcut {
    match id {
        ShortcutId::Dictate => &cosmic_shortcuts::DICTATE,
        ShortcutId::ReadAloud => &cosmic_shortcuts::READ_ALOUD,
    }
}

/// What the desktop calls shortcut `id`, under Super Voice's name.
fn description(id: ShortcutId) -> &'static str {
    match id {
        ShortcutId::Dictate => "Dictate",
        ShortcutId::ReadAloud => "Read aloud",
    }
}

/// The keys `id` asks for, in the portal's shortcut syntax: `LOGO+space` for
/// Super+Space. The desktop may give it other keys.
fn preferred_trigger(id: ShortcutId) -> String {
    let shortcut = shortcut(id);
    shortcut
        .modifiers
        .iter()
        .map(|modifier| match *modifier {
            "Super" => "LOGO".to_string(),
            "Ctrl" => "CTRL".to_string(),
            other => other.to_ascii_uppercase(),
        })
        .chain([shortcut.key.to_string()])
        .collect::<Vec<_>>()
        .join("+")
}

/// `BindShortcuts`' shortcuts: each id, with its description and the keys it
/// asks for.
fn bind_args(
    shortcuts: &[ShortcutId],
) -> Vec<(&'static str, HashMap<&'static str, Value<'static>>)> {
    shortcuts
        .iter()
        .map(|&id| {
            let mut properties = HashMap::new();
            properties.insert("description", Value::from(description(id)));
            properties.insert("preferred_trigger", Value::from(preferred_trigger(id)));
            (id.as_wire_str(), properties)
        })
        .collect()
}

/// The bound shortcuts in a `BindShortcuts` or `ListShortcuts` response.
/// A shortcut that is not Super Voice's is skipped.
fn bound_from(results: &HashMap<String, OwnedValue>) -> Vec<Bound> {
    let Some(shortcuts) = results.get("shortcuts").and_then(|v| v.try_clone().ok()) else {
        return Vec::new();
    };
    let Ok(shortcuts) = Vec::<(String, HashMap<String, OwnedValue>)>::try_from(shortcuts) else {
        warn!("GlobalShortcuts portal: shortcuts not in the expected form");
        return Vec::new();
    };
    let mut bound: Vec<Bound> = shortcuts
        .into_iter()
        .filter_map(|(id, properties)| {
            let id = id.parse::<ShortcutId>().ok()?;
            let keys = properties
                .get("trigger_description")
                .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
                .unwrap_or_default();
            Some(Bound { id, keys })
        })
        .collect();
    bound.sort_by_key(|b| ShortcutId::ALL.iter().position(|id| *id == b.id));
    bound
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each shortcut asks for the keys it has on COSMIC, in the portal's
    /// syntax.
    #[test]
    fn each_shortcut_asks_for_its_keys() {
        assert_eq!(preferred_trigger(ShortcutId::Dictate), "LOGO+space");
        assert_eq!(preferred_trigger(ShortcutId::ReadAloud), "LOGO+SHIFT+space");
    }

    #[test]
    fn a_bind_names_each_shortcut_by_its_wire_id() {
        let args = bind_args(&ShortcutId::ALL);
        let ids: Vec<_> = args.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, ["dictate", "read_aloud"]);
        assert_eq!(
            args[1].1.get("description"),
            Some(&Value::from("Read aloud"))
        );
    }

    #[test]
    fn session_tokens_are_not_reused() {
        let first = next_session_token();
        assert_ne!(first, next_session_token());
        // The portal accepts only [A-Za-z0-9_] in a token.
        assert!(
            first.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "{first}"
        );
    }
}
