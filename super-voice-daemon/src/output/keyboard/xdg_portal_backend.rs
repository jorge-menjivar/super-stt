// SPDX-License-Identifier: GPL-3.0-only

use anyhow::{Context, Result};
use futures::StreamExt;
use log::{debug, info, warn};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const REMOTE_DESKTOP_IFACE: &str = "org.freedesktop.portal.RemoteDesktop";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

const XKB_KEY_BACKSPACE: i32 = 0xFF08;

pub struct XdgPortalBackend {
    /// The `RemoteDesktop` portal proxy, built once and reused for every keysym
    /// press/release (audit 2 Tier 3 #1). `type_text` issues 2 keysym calls per
    /// char and the preview loop re-types the growing transcript each tick, so
    /// rebuilding the proxy per call meant constant D-Bus match-rule churn on the
    /// interactive path. The proxy holds its own clone of the async connection, so
    /// the portal session stays alive for the backend's lifetime.
    proxy: zbus::Proxy<'static>,
    session_path: OwnedObjectPath,
}

impl XdgPortalBackend {
    /// Check whether the `RemoteDesktop` portal interface is available.
    pub async fn is_available() -> bool {
        let Ok(conn) = zbus::Connection::session().await else {
            debug!("XDG Portal check: no session bus");
            return false;
        };

        let Ok(proxy) =
            zbus::Proxy::new(&conn, PORTAL_BUS, PORTAL_PATH, REMOTE_DESKTOP_IFACE).await
        else {
            debug!("XDG Portal check: failed to create proxy");
            return false;
        };

        match proxy.get_property::<u32>("AvailableDeviceTypes").await {
            Ok(types) => {
                debug!("XDG Portal check: AvailableDeviceTypes = {types}");
                // bit 0 = keyboard
                types & 1 != 0
            }
            Err(e) => {
                debug!("XDG Portal check: AvailableDeviceTypes failed: {e}");
                false
            }
        }
    }

    /// Create a new portal session (async — call from the daemon's async context).
    pub async fn new() -> Result<Self> {
        let conn = zbus::Connection::session()
            .await
            .context("Failed to connect to session D-Bus")?;

        let session_path = Self::setup_session(&conn).await?;

        // Build the RemoteDesktop proxy once and reuse it for every keysym
        // (audit 2 Tier 3 #1). The `&'static str` bus/path/interface constants
        // make this a `Proxy<'static>`, and it clones the connection internally.
        let proxy = zbus::Proxy::new(&conn, PORTAL_BUS, PORTAL_PATH, REMOTE_DESKTOP_IFACE)
            .await
            .context("Failed to create RemoteDesktop proxy")?;

        info!("XDG Desktop Portal write method ready (session: {session_path})");

        Ok(Self {
            proxy,
            session_path,
        })
    }

    async fn setup_session(conn: &zbus::Connection) -> Result<OwnedObjectPath> {
        let portal = zbus::Proxy::new(conn, PORTAL_BUS, PORTAL_PATH, REMOTE_DESKTOP_IFACE).await?;

        // Step 1: CreateSession
        let session_token = format!("superstt_s{}", std::process::id());

        let token = next_handle_token();
        let mut opts = HashMap::<&str, Value<'_>>::new();
        opts.insert("handle_token", Value::from(token.as_str()));
        opts.insert("session_handle_token", Value::from(session_token.as_str()));

        let results = portal_call(conn, &portal, "CreateSession", &token, &(opts,), 10).await?;

        let session_path: OwnedObjectPath = results
            .get("session_handle")
            .and_then(|v| TryInto::<String>::try_into(v.clone()).ok())
            .and_then(|s| OwnedObjectPath::try_from(s).ok())
            .context("No session_handle in CreateSession response")?;

        debug!("Portal session created: {session_path}");

        // Step 2: SelectDevices  (type 1 = keyboard)
        let token = next_handle_token();
        let mut opts = HashMap::<&str, Value<'_>>::new();
        opts.insert("handle_token", Value::from(token.as_str()));
        opts.insert("types", Value::U32(1));

        portal_call(
            conn,
            &portal,
            "SelectDevices",
            &token,
            &(session_path.as_ref(), opts),
            10,
        )
        .await?;

        debug!("Portal keyboard device selected");

        // Step 3: Start (may show authorization dialog)
        let token = next_handle_token();
        let mut opts = HashMap::<&str, Value<'_>>::new();
        opts.insert("handle_token", Value::from(token.as_str()));

        portal_call(
            conn,
            &portal,
            "Start",
            &token,
            &(session_path.as_ref(), "", opts),
            30,
        )
        .await?;

        info!("Portal session started — keyboard input authorised");
        Ok(session_path)
    }

    /// Send a keysym press or release via the portal.
    ///
    /// Issued directly on the backend's owned async `Connection`. The typing
    /// path is async end-to-end (audit Tier 3 #35), so this no longer spins up a
    /// one-shot thread + current-thread runtime per keysym just to escape a sync
    /// caller — it simply awaits the D-Bus call on the runtime.
    async fn notify_keysym(&self, keysym: i32, state: u32) -> Result<()> {
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        self.proxy
            .call_noreply(
                "NotifyKeyboardKeysym",
                &(self.session_path.as_ref(), options, keysym, state),
            )
            .await
            .map_err(|e| anyhow::anyhow!("NotifyKeyboardKeysym failed: {e}"))
    }

    /// Type each character as its own keysym, with no modifier around it.
    ///
    /// The compositor chooses the key and the level. GNOME and KDE press Shift
    /// for a keysym on a shifted level, and COSMIC commits a printable keysym
    /// as text. Holding Shift ourselves does not work everywhere: COSMIC
    /// treats each keysym as a tap and releases it at once, so the Shift was
    /// gone before the letter arrived and every capital came out lowercase.
    pub async fn type_text(&mut self, text: &str) -> Result<()> {
        for ch in text.chars() {
            let keysym = char_to_keysym(ch);
            self.notify_keysym(keysym, 1).await?;
            self.notify_keysym(keysym, 0).await?;
        }
        Ok(())
    }

    pub async fn backspace_n(&mut self, n: usize) -> Result<()> {
        for _ in 0..n {
            self.notify_keysym(XKB_KEY_BACKSPACE, 1).await?;
            self.notify_keysym(XKB_KEY_BACKSPACE, 0).await?;
        }
        Ok(())
    }
}

/// A `handle_token` no other request from this process is using.
fn next_handle_token() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("superstt_r{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The object path the portal gives a request that `unique_name` made with
/// `handle_token`, per the `org.freedesktop.portal.Request` naming scheme.
fn request_path(unique_name: &str, handle_token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/request/{sender}/{handle_token}")
}

/// Start listening for `Response` on one request path.
async fn subscribe_response(
    conn: &zbus::Connection,
    path: String,
) -> Result<zbus::proxy::SignalStream<'static>> {
    let proxy = zbus::Proxy::new(conn, PORTAL_BUS, path, REQUEST_IFACE).await?;
    Ok(proxy.receive_signal("Response").await?)
}

/// Call a portal method and wait for its Response signal.
///
/// Subscribes before calling. The portal can answer before the call has even
/// returned: on COSMIC, `Response` arrived 0.2 ms after the reply carrying the
/// request path, and a subscription made after that reply landed 0.7 ms too
/// late. The answer was lost and every step timed out, so the dialog at
/// `Start` almost never came up. The portal documents this race and its fix:
/// the caller sends `handle_token` in the method's options, which makes the
/// request path predictable before the call.
async fn portal_call(
    conn: &zbus::Connection,
    portal: &zbus::Proxy<'_>,
    method: &str,
    handle_token: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
    timeout_secs: u64,
) -> Result<HashMap<String, OwnedValue>> {
    let unique_name = conn
        .unique_name()
        .context("Session bus connection has no unique name")?;
    let expected = request_path(unique_name.as_str(), handle_token);
    let mut signals = subscribe_response(conn, expected.clone()).await?;

    let request_path: OwnedObjectPath = portal
        .call(method, body)
        .await
        .context(format!("Portal {method} call failed"))?;

    debug!("Portal {method}: request path = {request_path}");

    if request_path.as_str() != expected {
        // A portal older than 0.9 ignores `handle_token` and picks its own
        // path. Listening there now reopens the race, but it is all such a
        // portal allows.
        warn!("Portal {method}: request path {request_path} is not the predicted {expected}");
        signals = subscribe_response(conn, request_path.to_string()).await?;
    }

    let signal = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), signals.next())
        .await
        .context("Timeout waiting for portal Response")?
        .context("Signal stream ended without Response")?;

    let body = signal.body();
    let (code, results): (u32, HashMap<String, OwnedValue>) = body
        .deserialize()
        .context("Failed to deserialize portal Response")?;

    debug!("Portal {method}: response code = {code}");

    if code != 0 {
        return Err(anyhow::anyhow!(
            "Portal {method} failed (response code {code})"
        ));
    }

    Ok(results)
}

/// The keysym that produces `ch`: `S` for `S` and `!` for `!`, never the
/// unshifted key under it. That also keeps a character correct on a layout
/// other than US, where it may sit on a different key or level.
fn char_to_keysym(ch: char) -> i32 {
    let cp = ch as u32;
    // cp ≤ 0x10_FFFF (Unicode max). For the direct-map range (≤ 0xFF) and
    // the high-keysym range (0x0100_0000 | cp ≤ 0x011F_FFFF) the result
    // fits in i32; TryFrom with saturating fallback preserves behavior for
    // any realistic Unicode code point.
    match cp {
        0x20..=0x7E | 0xA0..=0xFF => i32::try_from(cp).unwrap_or(i32::MAX),
        0x0A => 0xFF0D,
        0x09 => 0xFF09,
        _ => i32::try_from(0x0100_0000_u32 | cp).unwrap_or(i32::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The portal names a request after its sender and `handle_token`. A path
    /// predicted wrong is a subscription on the wrong object, which is the race
    /// again: every step would wait out its timeout.
    #[test]
    fn request_path_follows_the_portal_naming_scheme() {
        assert_eq!(
            request_path(":1.676039", "superstt_r0"),
            "/org/freedesktop/portal/desktop/request/1_676039/superstt_r0"
        );
    }

    /// A shifted character is sent as itself. Sent as Shift plus the key
    /// under it, COSMIC typed `super stt` for `Super STT` and `1` for `!`.
    #[test]
    fn a_character_is_sent_as_its_own_keysym() {
        let cases = [
            ('a', 0x61),
            ('S', 0x53),
            ('!', 0x21),
            ('"', 0x22),
            ('~', 0x7E),
            ('é', 0xE9),
            ('€', 0x0100_20AC),
            ('\n', 0xFF0D),
            ('\t', 0xFF09),
        ];
        for (ch, keysym) in cases {
            assert_eq!(char_to_keysym(ch), keysym, "{ch:?}");
        }
    }

    #[test]
    fn handle_tokens_are_not_reused() {
        let first = next_handle_token();
        let second = next_handle_token();
        assert_ne!(first, second);
        // The portal accepts only [A-Za-z0-9_] in a token.
        assert!(
            first.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "{first}"
        );
    }
}
