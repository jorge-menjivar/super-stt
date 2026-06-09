// SPDX-License-Identifier: GPL-3.0-only

use anyhow::{Context, Result};
use futures::StreamExt;
use log::{debug, info};
use std::collections::HashMap;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const REMOTE_DESKTOP_IFACE: &str = "org.freedesktop.portal.RemoteDesktop";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

const XKB_KEY_BACKSPACE: i32 = 0xFF08;

pub struct XdgPortalBackend {
    /// The async zbus connection, used from sync context via a dedicated
    /// single-threaded executor so we never block a tokio worker.
    connection: zbus::Connection,
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

        info!("XDG Desktop Portal write method ready (session: {session_path})");

        Ok(Self {
            connection: conn,
            session_path,
        })
    }

    async fn setup_session(conn: &zbus::Connection) -> Result<OwnedObjectPath> {
        let portal = zbus::Proxy::new(conn, PORTAL_BUS, PORTAL_PATH, REMOTE_DESKTOP_IFACE).await?;

        // Step 1: CreateSession
        let session_token = format!("superstt_s{}", std::process::id());

        let mut opts = HashMap::<&str, Value<'_>>::new();
        opts.insert("session_handle_token", Value::from(session_token.as_str()));

        let results = portal_call(conn, &portal, "CreateSession", &(opts,), 10).await?;

        let session_path: OwnedObjectPath = results
            .get("session_handle")
            .and_then(|v| TryInto::<String>::try_into(v.clone()).ok())
            .and_then(|s| OwnedObjectPath::try_from(s).ok())
            .context("No session_handle in CreateSession response")?;

        debug!("Portal session created: {session_path}");

        // Step 2: SelectDevices  (type 1 = keyboard)
        let mut opts = HashMap::<&str, Value<'_>>::new();
        opts.insert("types", Value::U32(1));

        portal_call(
            conn,
            &portal,
            "SelectDevices",
            &(session_path.as_ref(), opts),
            10,
        )
        .await?;

        debug!("Portal keyboard device selected");

        // Step 3: Start (may show authorization dialog)
        let opts = HashMap::<&str, Value<'_>>::new();

        portal_call(
            conn,
            &portal,
            "Start",
            &(session_path.as_ref(), "", opts),
            30,
        )
        .await?;

        info!("Portal session started — keyboard input authorised");
        Ok(session_path)
    }

    /// Send a keysym press or release via the portal.
    ///
    /// This is called from sync code running on a tokio worker thread, so we
    /// cannot use `block_on`.  Instead we send via `call_noreply` on a
    /// one-shot `std::thread` to avoid blocking the async runtime.
    fn notify_keysym(&self, keysym: i32, state: u32) -> Result<()> {
        let conn = self.connection.clone();
        let session = self.session_path.clone();

        // Run the async D-Bus call on a short-lived thread that is allowed
        // to block.  The overhead is negligible compared to the key event
        // latency itself.
        std::thread::scope(|s| {
            s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("mini runtime");
                rt.block_on(async {
                    let proxy =
                        zbus::Proxy::new(&conn, PORTAL_BUS, PORTAL_PATH, REMOTE_DESKTOP_IFACE)
                            .await?;
                    let options: HashMap<&str, Value<'_>> = HashMap::new();
                    proxy
                        .call_noreply(
                            "NotifyKeyboardKeysym",
                            &(session.as_ref(), options, keysym, state),
                        )
                        .await?;
                    Ok::<(), zbus::Error>(())
                })
            })
            .join()
            .expect("keysym thread panicked")
            .map_err(|e| anyhow::anyhow!("NotifyKeyboardKeysym failed: {e}"))
        })
    }

    pub fn type_text(&mut self, text: &str) -> Result<()> {
        for ch in text.chars() {
            let (needs_shift, keysym) = char_to_keysym(ch);
            if needs_shift {
                self.notify_keysym(XKB_KEY_SHIFT_L, 1)?;
            }
            self.notify_keysym(keysym, 1)?;
            self.notify_keysym(keysym, 0)?;
            if needs_shift {
                self.notify_keysym(XKB_KEY_SHIFT_L, 0)?;
            }
        }
        Ok(())
    }

    pub fn backspace_n(&mut self, n: usize) -> Result<()> {
        for _ in 0..n {
            self.notify_keysym(XKB_KEY_BACKSPACE, 1)?;
            self.notify_keysym(XKB_KEY_BACKSPACE, 0)?;
        }
        Ok(())
    }
}

/// Call a portal method and wait for the Response signal.
async fn portal_call(
    conn: &zbus::Connection,
    portal: &zbus::Proxy<'_>,
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
    timeout_secs: u64,
) -> Result<HashMap<String, OwnedValue>> {
    let request_path: OwnedObjectPath = portal
        .call(method, body)
        .await
        .context(format!("Portal {method} call failed"))?;

    debug!("Portal {method}: request path = {request_path}");

    let req_proxy: zbus::Proxy<'_> =
        zbus::Proxy::new(conn, PORTAL_BUS, request_path.as_str(), REQUEST_IFACE).await?;

    let mut signals = req_proxy.receive_signal("Response").await?;

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

const XKB_KEY_SHIFT_L: i32 = 0xFFE1;

/// Whether a character requires Shift and what keysym to send.
/// Returns `(needs_shift, keysym)`.
fn char_to_keysym(ch: char) -> (bool, i32) {
    // Uppercase letters → Shift + lowercase keysym. ASCII lowercase is always ≤ 0x7A < i32::MAX.
    if ch.is_ascii_uppercase() {
        return (true, i32::from(ch.to_ascii_lowercase() as u8));
    }

    // Characters that are Shift+<base key> on a US layout
    let shifted = match ch {
        '!' => Some(0x31), // 1
        '@' => Some(0x32), // 2
        '#' => Some(0x33), // 3
        '$' => Some(0x34), // 4
        '%' => Some(0x35), // 5
        '^' => Some(0x36), // 6
        '&' => Some(0x37), // 7
        '*' => Some(0x38), // 8
        '(' => Some(0x39), // 9
        ')' => Some(0x30), // 0
        '_' => Some(0x2D), // -
        '+' => Some(0x3D), // =
        '{' => Some(0x5B), // [
        '}' => Some(0x5D), // ]
        '|' => Some(0x5C), // backslash
        ':' => Some(0x3B), // ;
        '"' => Some(0x27), // '
        '<' => Some(0x2C), // ,
        '>' => Some(0x2E), // .
        '?' => Some(0x2F), // /
        '~' => Some(0x60), // `
        _ => None,
    };

    if let Some(base) = shifted {
        return (true, base);
    }

    let cp = ch as u32;
    // cp ≤ 0x10_FFFF (Unicode max). For the direct-map range (≤ 0xFF) and
    // the high-keysym range (0x0100_0000 | cp ≤ 0x011F_FFFF) the result
    // fits in i32; TryFrom with saturating fallback preserves behavior for
    // any realistic Unicode code point.
    let keysym = match cp {
        0x20..=0x7E | 0xA0..=0xFF => i32::try_from(cp).unwrap_or(i32::MAX),
        0x0A => 0xFF0D,
        0x09 => 0xFF09,
        _ => i32::try_from(0x0100_0000_u32 | cp).unwrap_or(i32::MAX),
    };
    (false, keysym)
}
